//! 壓 BAR：在舊標題出現在標題條的時段，整張蓋上新的 BAR 圖（1920×1080 透明 PNG）。
//!
//! 時段用掃描時的標題條 OCR（每 2 秒一張）找：OCR 文字跟「要換掉的舊標題」夠像就算出現。
//! OCR 只有 2 秒一張，所以每個時段回傳開頭／結尾「可能在哪一段格數之間」，
//! 出片前再逐格比對標題條找準切換的那一格（app 端 `refine_bar_edge`）。

use super::titles::{bigrams, norm};
use std::collections::BTreeMap;

/// OCR 文字有幾成的字組出現在舊標題裡才算同一個標題（跟對段落一樣用 60%）
const MATCH: f32 = 0.6;
/// 讀不出字（轉場、沒有標題條）的 OCR 最多跨過幾張，不打斷同一個時段
const BRIDGE: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleRun {
    /// 舊標題出現的那一格在 [start_lo, start_hi] 之間
    pub start_lo: usize,
    pub start_hi: usize,
    /// 舊標題最後一格在 [end_lo, end_hi] 之間
    pub end_lo: usize,
    pub end_hi: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Sample {
    Match,
    Other,
    /// 字太少，看不出是哪個標題
    Unknown,
}

fn classify(old: &[(char, char)], text: &str) -> Sample {
    let b = bigrams(&norm(text));
    if b.len() < 6 {
        return Sample::Unknown;
    }
    let hit = b.iter().filter(|g| old.contains(g)).count() as f32 / b.len() as f32;
    if hit >= MATCH { Sample::Match } else { Sample::Other }
}

/// `from..=to` 這段格數裡，舊標題出現的每個時段（照時間順序）
pub fn title_runs(ocr: &BTreeMap<usize, String>, old: &str, from: usize, to: usize) -> Vec<TitleRun> {
    let old = bigrams(&norm(old));
    if old.len() < 3 || from > to {
        return Vec::new();
    }
    let samples: Vec<(usize, Sample)> = ocr.range(from..=to).map(|(&k, t)| (k, classify(&old, t))).collect();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < samples.len() {
        if samples[i].1 != Sample::Match {
            i += 1;
            continue;
        }
        // 往後延伸：Match 接著，或中間最多 BRIDGE 張讀不出字再接 Match
        let first = i;
        let mut last = i;
        let mut j = i + 1;
        while j < samples.len() {
            match samples[j].1 {
                Sample::Match => {
                    last = j;
                    j += 1;
                }
                Sample::Unknown => {
                    let gap_end = (j..samples.len()).find(|&k| samples[k].1 != Sample::Unknown).unwrap_or(samples.len());
                    if gap_end - j <= BRIDGE && gap_end < samples.len() && samples[gap_end].1 == Sample::Match {
                        j = gap_end;
                    } else {
                        break;
                    }
                }
                Sample::Other => break,
            }
        }
        // 開頭：從上一張的後一格起；結尾：到下一張的前一格（窗口 2 秒）。
        // 鄰居讀不出字時，標題可能還在，再多看一張（窗口 4 秒）
        let before = |k: usize| if samples[k].1 == Sample::Unknown && k > 0 { k - 1 } else { k };
        let after = |k: usize| if samples[k].1 == Sample::Unknown && k + 1 < samples.len() { k + 1 } else { k };
        let prev = if first > 0 { samples[before(first - 1)].0 + 1 } else { from };
        let next = if last + 1 < samples.len() { samples[after(last + 1)].0.saturating_sub(1) } else { to };
        runs.push(TitleRun { start_lo: prev.max(from), start_hi: samples[first].0, end_lo: samples[last].0, end_hi: next.min(to) });
        i = last + 1;
    }
    runs
}

/// 標題條逐格的差異（第 k 個＝第 k 格跟第 k-1 格差多少，第 0 個是 0）→ 換標題的那一格（差最多的）
pub fn change_frame(diffs: &[f32]) -> Option<usize> {
    let (k, d) = diffs.iter().enumerate().skip(1).max_by(|a, b| a.1.total_cmp(b.1))?;
    (*d > 0.0).then_some(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "河口湖殺人頭七破案？丈夫員工成「刺客」跨海殺妻.台日攜手偵辦揭暗黑產業鏈！？";

    fn ocr(items: &[(usize, &str)]) -> BTreeMap<usize, String> {
        items.iter().map(|(k, t)| (*k, t.to_string())).collect()
    }

    #[test]
    fn finds_each_run_with_edge_windows() {
        let other = "徐佳青涉貪卻獲邀僑委晚宴高歌逐桌敬酒";
        let o = ocr(&[
            (0, other),
            (60, "河口湖殺人頭七破案?丈夫員工成\"刺客\"跨海殺妻"),
            (120, "河口湖殺人頭七破案?丈夫員工成刺客跨海殺妻.台日攜手偵辦"),
            (180, other),
            (240, "河口湖殺人頭七破案丈夫員工成刺客跨海殺妻台日攜手偵辦揭暗黑產業鏈"),
            (300, other),
        ]);
        let r = title_runs(&o, OLD, 0, 400);
        assert_eq!(r, vec![TitleRun { start_lo: 1, start_hi: 60, end_lo: 120, end_hi: 179 }, TitleRun { start_lo: 181, start_hi: 240, end_lo: 240, end_hi: 299 }]);
    }

    #[test]
    fn one_unreadable_sample_does_not_split_but_two_do() {
        let o = ocr(&[(0, OLD), (60, ""), (120, OLD), (180, "?"), (240, "!!"), (300, OLD)]);
        let r = title_runs(&o, OLD, 0, 400);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].start_hi, r[0].end_lo), (0, 120));
        assert_eq!((r[1].start_hi, r[1].end_lo), (300, 300));
        // 沒有「確定不是」的鄰居：窗口到段落頭尾
        assert_eq!((r[0].start_lo, r[1].end_hi), (0, 400));
    }

    #[test]
    fn similar_but_different_title_is_not_matched() {
        let o = ocr(&[(0, "河口湖事件井女踏上死亡之旅秘辛曝!新婚丈夫佈局派出冷血女刺客跨海殺妻!?")]);
        assert!(title_runs(&o, OLD, 0, 100).is_empty());
    }

    #[test]
    fn change_frame_is_biggest_jump() {
        assert_eq!(change_frame(&[0.0, 0.1, 0.2, 9.0, 0.3]), Some(3));
        assert_eq!(change_frame(&[0.0, 0.0]), None);
        assert_eq!(change_frame(&[]), None);
    }
}
