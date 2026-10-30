//! 側錄檔接縫：下一個 10 分鐘檔開頭偶爾會多錄到上一個檔最後約 1 秒多（同一段畫面和聲音出現兩次）。
//!
//! 用聲音找：講話的波形幾秒內不會一模一樣再出現，下一檔開頭 0.25 秒的波形如果在上一檔結尾逐樣本對得上，
//! 就是重複錄到，對上的位置到上一檔結尾就是重疊長度。畫面不可靠（棚內鏡頭、靜止畫面本來就會連續好幾格一樣）。
//! 實測（10/29 23:30→23:40 真接縫、假造 0.2～5 秒重疊）：真接縫最像 0.60，重疊 1.00（音量 -10%＋雜訊 0.86）。
//!
//! 跳過（中間少錄）聲音比不到，只能用檔名時間＋長度算，見 [`gap_secs`]。

use super::cache::name_timestamp;

/// 比對用的取樣率（ffmpeg 轉單聲道 8kHz s16le）
pub const SR: usize = 8000;
/// 取上一檔結尾幾秒來找（重疊最長能抓到這麼長）
pub const TAIL_SECS: f64 = 4.0;
/// 取下一檔開頭幾秒（找的時候用前 PROBE_SECS，剩下的拿來確認）
pub const HEAD_SECS: f64 = 2.0;
/// 拿下一檔開頭多長去找；比這短的重疊抓不到（實際遇到的是 1 秒多）
pub const PROBE_SECS: f64 = 0.25;
/// 波形相關係數到這麼高才算重複（真接縫實測最高 0.60）
pub const OVERLAP_NCC: f64 = 0.9;
/// 重疊的整段再確認一次（重新編碼過的重複實測 0.86）
pub const CONFIRM_NCC: f64 = 0.8;
/// 開頭太安靜（RMS 低於這個）比不出來
pub const SILENT_RMS: f64 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seam {
    /// 下一檔開頭重複了上一檔最後這麼多秒（score＝波形相關係數）
    Overlap { secs: f64, score: f64 },
    /// 沒有重複（best＝最像的地方有多像）
    Clean { best: f64 },
    /// 下一檔開頭太安靜，比不出來
    Silent,
}

impl Seam {
    /// 上一檔結尾要拿掉幾格
    pub fn trim_frames(&self) -> usize {
        match self {
            Seam::Overlap { secs, .. } => (secs * super::FPS).round() as usize,
            _ => 0,
        }
    }
}

fn rms(x: &[i16]) -> f64 {
    (x.iter().map(|&v| v as f64 * v as f64).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn ncc(a: &[i16], b: &[i16]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let (ma, mb) = (a[..n].iter().map(|&v| v as f64).sum::<f64>() / n as f64, b[..n].iter().map(|&v| v as f64).sum::<f64>() / n as f64);
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (&x, &y) in a[..n].iter().zip(&b[..n]) {
        let (x, y) = (x as f64 - ma, y as f64 - mb);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    let den = (da * db).sqrt();
    if den > 0.0 { num / den } else { 0.0 }
}

/// `tail`＝上一檔最後幾秒、`head`＝下一檔最前幾秒（都是 [`SR`] 單聲道）。
/// 拿 head 開頭 [`PROBE_SECS`] 在 tail 裡逐樣本滑動找，對到的位置到 tail 結尾＝重疊長度。
/// 下一檔開頭常有一小段完全沒聲音（重複錄到的也一樣是那段靜音），就往後找第一段有聲音的來比。
pub fn overlap(tail: &[i16], head: &[i16]) -> Seam {
    let w = (PROBE_SECS * SR as f64) as usize;
    if head.len() < w || tail.len() < w {
        return Seam::Clean { best: 0.0 };
    }
    // 開頭第一段有聲音的位置（每 50ms 試一次）
    let step = SR / 20;
    let Some(o) = (0..=head.len() - w).step_by(step).find(|&o| rms(&head[o..o + w]) >= SILENT_RMS) else {
        return Seam::Silent;
    };
    let probe = &head[o..o + w];
    let mp = probe.iter().map(|&v| v as f64).sum::<f64>() / w as f64;
    let pc: Vec<f64> = probe.iter().map(|&v| v as f64 - mp).collect();
    let pn = pc.iter().map(|v| v * v).sum::<f64>().sqrt();
    // tail 的滑動和、平方和
    let mut sum = vec![0.0f64; tail.len() + 1];
    let mut sq = vec![0.0f64; tail.len() + 1];
    for (i, &v) in tail.iter().enumerate() {
        sum[i + 1] = sum[i] + v as f64;
        sq[i + 1] = sq[i] + v as f64 * v as f64;
    }
    let mut best = (f64::MIN, 0usize);
    for p in 0..=tail.len() - w {
        let s = sum[p + w] - sum[p];
        let var = (sq[p + w] - sq[p]) - s * s / w as f64;
        if var <= 0.0 {
            continue;
        }
        // pc 已經去平均，Σ(x-mx)(y-my)＝Σx(y-my)
        let num: f64 = tail[p..p + w].iter().zip(&pc).map(|(&x, y)| x as f64 * y).sum();
        let score = num / (var.sqrt() * pn);
        if score > best.0 {
            best = (score, p);
        }
    }
    let (score, p) = best;
    if score < OVERLAP_NCC {
        return Seam::Clean { best: score.max(0.0) };
    }
    // 對到的位置往前推 o＝重疊在上一檔的起點；整段重疊都要對得上（不只那一小段像）
    let Some(start) = p.checked_sub(o) else { return Seam::Clean { best: score } };
    let d = tail.len() - start;
    if ncc(&tail[p..], &head[o..(o + tail.len() - p).min(head.len())]) < CONFIRM_NCC {
        return Seam::Clean { best: score };
    }
    Seam::Overlap { secs: d as f64 / SR as f64, score }
}

/// 畫面再確認一次（聲音可能被持續的配樂、音效騙到；10/25 23:30 聲音說重複 0.25 秒，畫面只像 2 格）。
/// `tail`／`head`＝上一檔最後幾格、下一檔最前幾格的縮小灰階（同樣大小），`guess`＝聲音算出的重疊格數。
/// 在 guess±[`VIDEO_SLACK`] 格裡找「下一檔開頭 k 格＝上一檔最後 k 格」最像的 k；平均每點差不到 [`VIDEO_DIFF`] 才算 → 重疊格數
pub fn video_overlap(tail: &[Vec<u8>], head: &[Vec<u8>], guess: usize) -> Option<usize> {
    let diff = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(x, y)| (*x as f32 - *y as f32).abs()).sum::<f32>() / a.len().max(1) as f32;
    let lo = guess.saturating_sub(VIDEO_SLACK).max(1);
    let hi = (guess + VIDEO_SLACK).min(tail.len()).min(head.len());
    (lo..=hi)
        .map(|k| (k, (0..k).map(|j| diff(&tail[tail.len() - k + j], &head[j])).sum::<f32>() / k as f32))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|&(_, d)| d < VIDEO_DIFF)
        .map(|(k, _)| k)
}

/// 開頭沒聲音比不了時只看畫面：k＝1..=tail 長度裡「下一檔開頭 k 格＝上一檔最後 k 格」最像的 k。
/// 靜止畫面（黑畫面、圖卡）每個 k 都像，所以要最像的 k 明顯比其他 k（差 2 格以上的）好才算
pub fn video_only_overlap(tail: &[Vec<u8>], head: &[Vec<u8>]) -> Option<usize> {
    let diff = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(x, y)| (*x as f32 - *y as f32).abs()).sum::<f32>() / a.len().max(1) as f32;
    let max = tail.len().min(head.len());
    let scores: Vec<(usize, f32)> = (1..=max).map(|k| (k, (0..k).map(|j| diff(&tail[tail.len() - k + j], &head[j])).sum::<f32>() / k as f32)).collect();
    let &(k, best) = scores.iter().min_by(|a, b| a.1.total_cmp(&b.1))?;
    let others = scores.iter().filter(|(j, _)| j.abs_diff(k) >= 2).map(|s| s.1).fold(f32::MAX, f32::min);
    (best < VIDEO_DIFF && others > VIDEO_DIFF * 2.0 && k >= VIDEO_ONLY_MIN).then_some(k)
}

/// 只看畫面時少於這麼多格不算：一般 10 分鐘接縫本來就重疊約 2 格（每檔 600.07 秒、間隔 600 秒），
/// 看不出來，畫面比對也時有時無；要抓的是側錄機重開那種 0.5～1.4 秒
pub const VIDEO_ONLY_MIN: usize = 6;

/// 只看畫面時最長找幾格（實測重疊最長 43 格）
pub const VIDEO_ONLY_MAX: usize = 60;

/// 畫面確認：聲音算的格數前後多找幾格（聲音和畫面的結尾可能差一兩格）
pub const VIDEO_SLACK: usize = 4;
/// 64×36 灰階平均每點差多少以內算同一格（重複錄到的實測 0.5，接著播的下一格 10 以上）
pub const VIDEO_DIFF: f32 = 2.0;

/// 檔名時間：下一檔開始 −（上一檔開始＋長度）。正＝中間少錄、負＝重疊。
/// 檔名只精確到秒，±1 秒內是誤差。檔名沒有時間就 None
pub fn gap_secs(prev_name: &str, prev_secs: f64, next_name: &str) -> Option<f64> {
    let t = |name: &str| {
        let s = name_timestamp(name)?;
        let n = |a: usize, b: usize| s[a..b].parse::<i64>().ok();
        let days = days_from_civil(n(0, 4)?, n(4, 6)?, n(6, 8)?);
        Some((days * 86400 + n(8, 10)? * 3600 + n(10, 12)? * 60 + n(12, 14)?) as f64)
    };
    Some(t(next_name)? - (t(prev_name)? + prev_secs))
}

/// 西元日期 → 1970-01-01 起第幾天（跨月、跨年的接縫也要算得對）
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 像講話的聲音：亂數雜訊乘上忽大忽小的包絡（固定種子，每次一樣）
    fn speech(n: usize, seed: u64) -> Vec<i16> {
        let mut s = seed;
        (0..n)
            .map(|i| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let noise = ((s >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0;
                let env = 0.3 + 0.7 * ((i as f64 / SR as f64) * 3.0 + seed as f64).sin().abs();
                (noise * env * 8000.0) as i16
            })
            .collect()
    }

    fn secs(s: f64) -> usize {
        (s * SR as f64) as usize
    }

    #[test]
    fn clean_join_is_not_an_overlap() {
        let tail = speech(secs(TAIL_SECS), 1);
        let head = speech(secs(HEAD_SECS), 2);
        assert!(matches!(overlap(&tail, &head), Seam::Clean { .. }));
    }

    #[test]
    fn repeated_end_is_found_with_its_length() {
        let tail = speech(secs(TAIL_SECS), 3);
        for k in [0.3, 1.2771, 1.5, 3.0] {
            let n = secs(k);
            let head: Vec<i16> = tail[tail.len() - n..].iter().copied().chain(speech(secs(HEAD_SECS), 4)).take(secs(HEAD_SECS)).collect();
            match overlap(&tail, &head) {
                Seam::Overlap { secs: d, .. } => assert!((d - n as f64 / SR as f64).abs() < 0.001, "{k}: {d}"),
                other => panic!("{k}: {other:?}"),
            }
        }
    }

    #[test]
    fn re_encoded_repeat_is_still_found() {
        let tail = speech(secs(TAIL_SECS), 5);
        let n = secs(1.2);
        let noise = speech(n, 9);
        let head: Vec<i16> = tail[tail.len() - n..]
            .iter()
            .zip(&noise)
            .map(|(&v, &z)| (v as f64 * 0.9 + z as f64 * 0.04) as i16)
            .chain(speech(secs(HEAD_SECS), 6))
            .take(secs(HEAD_SECS))
            .collect();
        assert!(matches!(overlap(&tail, &head), Seam::Overlap { secs, .. } if (secs - 1.2).abs() < 0.001));
    }

    #[test]
    fn silence_at_the_start_is_skipped() {
        let tail = speech(secs(TAIL_SECS), 11);
        let n = secs(1.3);
        // 上一檔最後 1.3 秒的前 0.4 秒是靜音，下一檔開頭重複了這 1.3 秒
        let mut tail = tail;
        let len = tail.len();
        tail[len - n..len - n + secs(0.4)].fill(0);
        let head: Vec<i16> = tail[len - n..].iter().copied().chain(speech(secs(HEAD_SECS), 12)).take(secs(HEAD_SECS)).collect();
        assert!(matches!(overlap(&tail, &head), Seam::Overlap { secs, .. } if (secs - 1.3).abs() < 0.001));
    }

    #[test]
    fn video_confirms_the_overlap_length() {
        let frame = |v: u8| vec![v; 16];
        // 上一檔最後 10 格是 0..10，下一檔開頭重複了最後 6 格（4..10）再接新的
        let tail: Vec<Vec<u8>> = (0..10).map(|i| frame(i * 12)).collect();
        let head: Vec<Vec<u8>> = (4..10).chain(10..14).map(|i| frame(i * 12)).collect();
        assert_eq!(video_overlap(&tail, &head, 7), Some(6));
        // 沒有重複：下一檔接著播新的
        let head: Vec<Vec<u8>> = (10..20).map(|i| frame(i * 12)).collect();
        assert_eq!(video_overlap(&tail, &head, 6), None);
    }

    #[test]
    fn video_alone_needs_a_moving_picture() {
        let frame = |v: u8| vec![v; 16];
        let tail: Vec<Vec<u8>> = (0..10).map(|i| frame(i * 12)).collect();
        let head: Vec<Vec<u8>> = (4..10).chain(10..14).map(|i| frame(i * 12)).collect();
        assert_eq!(video_only_overlap(&tail, &head), Some(6));
        // 只重疊 2 格（一般接縫）不算
        let head: Vec<Vec<u8>> = (8..10).chain(10..18).map(|i| frame(i * 12)).collect();
        assert_eq!(video_only_overlap(&tail, &head), None);
        // 黑畫面接黑畫面：每個 k 都一樣像，不能說重疊幾格
        let black: Vec<Vec<u8>> = (0..10).map(|_| frame(16)).collect();
        assert_eq!(video_only_overlap(&black, &black), None);
    }

    #[test]
    fn silent_start_cannot_be_judged() {
        let tail = speech(secs(TAIL_SECS), 7);
        assert_eq!(overlap(&tail, &vec![3i16; secs(HEAD_SECS)]), Seam::Silent);
    }

    #[test]
    fn trim_is_in_frames() {
        assert_eq!(Seam::Overlap { secs: 1.2, score: 1.0 }.trim_frames(), 36);
        assert_eq!(Seam::Clean { best: 0.5 }.trim_frames(), 0);
    }

    #[test]
    fn gap_from_file_names() {
        let a = "#1_Channel 3---2026_10_29---23_30_41---H264  8 bit---198---split010.mp4";
        let b = "#1_Channel 3---2026_10_29---23_40_41---H264  8 bit---172---split011.mp4";
        assert!((gap_secs(a, 600.0, b).unwrap()).abs() < 1e-9);
        // 跨午夜
        let c = "#1_Channel 3---2026_10_29---23_58_22---H264  8 bit---770.mp4";
        let d = "#1_Channel 3---2026_10_30---00_08_22---H264  8 bit---1---split001.mp4";
        assert!((gap_secs(c, 598.0, d).unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(gap_secs("a.mp4", 600.0, b), None);
    }
}
