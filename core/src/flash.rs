//! 從每格平均亮度找出閃白，給全集第三、六段自動抓切點用。
//!
//! 第三、六段的完整單片一定是「當天帶子 ⚡ YT ｜ promo」，閃白之後都不要。閃白（xfade
//! fadewhite、剪映的閃白）在亮度上的形狀固定：前一段畫面連續幾格一路變亮到接近全白，
//! 再淡出到下一段。只是畫面很亮（白色字卡）不算——它沒有逐格變亮的過程。
//!
//! 亮度由 ffmpeg `signalstats` 的 YAVG 提供（8-bit，limited range 白＝235，full range 白＝255）。

/// 頂峰至少要這麼亮才算接近全白
const WHITE_MIN: f64 = 200.0;
/// 從乾淨畫面到頂峰至少要變亮這麼多
const MIN_RISE: f64 = 50.0;
/// 每格亮度增加超過這個值才算還在變亮（畫面本身的雜訊遠小於閃白每格的變化）
const STEP_EPS: f64 = 0.25;
/// 往回找乾淨畫面最多退幾格（約 2 秒），避免一路退進一般畫面
const MAX_RAMP_FRAMES: usize = 60;
/// 兩個頂峰相距不到這麼多秒算同一個閃白
const MERGE_SECS: f64 = 1.0;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FlashCandidate {
    /// 最白那格的時間
    pub peak: f64,
    /// 閃白前最後一格乾淨畫面的時間
    pub clean_frame: f64,
    /// 切點：閃白開始變亮的第一格時間。輸出用 end＝這個值，乾淨那格是最後一格。
    pub cut: f64,
}

/// 解析 ffmpeg `metadata=print:key=lavfi.signalstats.YAVG` 的輸出，回傳 (秒, 亮度)。
/// `offset` 加在每格時間上（`-ss` 放在 `-i` 前時，時間從 0 開始算）。
pub fn parse_yavg(output: &str, offset: f64) -> Vec<(f64, f64)> {
    let mut frames = Vec::new();
    let mut time = None;
    for line in output.lines() {
        if let Some(rest) = line.split("pts_time:").nth(1) {
            time = rest.split_whitespace().next().and_then(|t| t.parse::<f64>().ok());
        } else if let Some(v) = line.trim().strip_prefix("lavfi.signalstats.YAVG=") {
            if let (Some(t), Ok(y)) = (time.take(), v.trim().parse::<f64>()) {
                frames.push((t + offset, y));
            }
        }
    }
    frames
}

/// 依時間順序列出所有像閃白的位置。
pub fn find_flashes(frames: &[(f64, f64)]) -> Vec<FlashCandidate> {
    let mut out: Vec<FlashCandidate> = Vec::new();
    if frames.len() < 3 {
        return out;
    }
    let y = |i: usize| frames[i].1;
    for i in 1..frames.len() - 1 {
        if y(i) < WHITE_MIN || y(i) < y(i - 1) || y(i) < y(i + 1) {
            continue;
        }
        let mut j = i;
        while j > 0 && i - j < MAX_RAMP_FRAMES && y(j) - y(j - 1) > STEP_EPS {
            j -= 1;
        }
        if i - j < 2 || y(i) - y(j) < MIN_RISE {
            continue;
        }
        let peak = frames[i].0;
        if out.last().is_some_and(|c| peak - c.peak < MERGE_SECS) {
            continue;
        }
        out.push(FlashCandidate { peak, clean_frame: frames[j].0, cut: frames[j + 1].0 });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f64 = 29.97;

    fn frames(values: &[f64]) -> Vec<(f64, f64)> {
        values.iter().enumerate().map(|(i, &y)| (i as f64 / FPS, y)).collect()
    }

    /// 一般畫面（亮度在 base 附近小幅晃動）
    fn content(n: usize, base: f64) -> Vec<f64> {
        (0..n).map(|i| base + [0.0, 0.2, -0.1, 0.15][i % 4]).collect()
    }

    /// 閃白：從 from 逐格變亮到 235，再逐格淡到 to
    fn flash(from: f64, to: f64) -> Vec<f64> {
        let up: Vec<f64> = (1..=7).map(|k| from + (235.0 - from) * (k as f64 / 7.0).powi(2)).collect();
        let down: Vec<f64> = (1..=7).map(|k| 235.0 - (235.0 - to) * (k as f64 / 7.0)).collect();
        [up, down].concat()
    }

    #[test]
    fn finds_the_last_clean_frame_before_the_flash() {
        let mut v = content(90, 110.0);
        v.extend(flash(110.0, 90.0));
        v.extend(content(60, 90.0));
        let r = find_flashes(&frames(&v));
        assert_eq!(r.len(), 1);
        // 第 89 格是最後一格一般畫面，第 90 格開始變亮
        assert!((r[0].clean_frame - 89.0 / FPS).abs() < 1e-9, "{r:?}");
        assert!((r[0].cut - 90.0 / FPS).abs() < 1e-9, "{r:?}");
        assert!((r[0].peak - 96.0 / FPS).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn a_white_card_without_a_ramp_is_not_a_flash() {
        let mut v = content(30, 110.0);
        v.extend(std::iter::repeat_n(235.0, 30));
        v.extend(content(30, 110.0));
        assert!(find_flashes(&frames(&v)).is_empty());
    }

    #[test]
    fn a_bright_scene_that_never_gets_white_is_not_a_flash() {
        let mut v = content(30, 120.0);
        v.extend((1..=10).map(|k| 120.0 + k as f64 * 7.0)); // 升到 190
        v.extend(content(30, 190.0));
        assert!(find_flashes(&frames(&v)).is_empty());
    }

    #[test]
    fn several_flashes_come_back_in_order() {
        let mut v = content(60, 100.0);
        v.extend(flash(100.0, 80.0));
        v.extend(content(120, 80.0));
        v.extend(flash(80.0, 130.0));
        v.extend(content(30, 130.0));
        let r = find_flashes(&frames(&v));
        assert_eq!(r.len(), 2);
        assert!(r[0].peak < r[1].peak);
        assert!((r[0].cut - 60.0 / FPS).abs() < 1e-9);
    }

    #[test]
    fn parses_metadata_print_output_with_offset() {
        let out = "frame:0    pts:0       pts_time:0\nlavfi.signalstats.YAVG=16.5\n\
                   frame:1    pts:1001    pts_time:0.0333667\nlavfi.signalstats.YAVG=235\n";
        let f = parse_yavg(out, 100.0);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0], (100.0, 16.5));
        assert!((f[1].0 - 100.0333667).abs() < 1e-9);
        assert_eq!(f[1].1, 235.0);
    }
}
