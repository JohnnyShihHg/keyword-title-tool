//! 從文稿（`render` 的成品 txt）取出每段上 YouTube 用的標題和說明。
//!
//! 標題＝段標題行（`段標題【關鍵時刻】20261026-1張炤和`），說明＝標題行之後到 `=======`
//! 的所有行（標題列表＋文稿裡的 #關鍵字，原樣放進說明）。後台「標記」不從文稿拿，
//! 由使用者在自動切段頁每段自己填（見 [`parse_tags`]）。

use crate::parse::{hashtags, is_keywords_heading, is_keywords_line, keywords_in_line};
use serde::Serialize;

/// YouTube 標題上限 100 字
pub const TITLE_MAX: usize = 100;
/// 說明上限 5000 位元組
pub const DESCRIPTION_MAX_BYTES: usize = 5000;
/// 後台標籤合計上限 500 字（含逗號，有空白的標籤 YouTube 會加引號）
pub const TAGS_MAX: usize = 500;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct VideoMeta {
    pub number: usize,
    pub title: String,
    pub description: String,
    /// 文稿這段的「後臺關鍵字」（沒有那個區塊就用 #關鍵字）
    pub keywords: Vec<String>,
    /// 超過上限或含不允許字元時的提醒（仍可上傳，已自動截掉／換掉）
    pub warnings: Vec<String>,
}

/// 「第一段」「第1段」→ 1
fn segment_label(line: &str) -> Option<usize> {
    let t = line.trim().strip_prefix('第')?.strip_suffix('段')?;
    "一二三四五六七八九".find(t).map(|i| i / 3 + 1).or_else(|| t.parse().ok())
}

/// YouTube 不收 `<` `>`，換成全形
fn clean(s: &str) -> String {
    s.replace('<', "＜").replace('>', "＞")
}

/// 文稿 → 每段的標題與說明（只列有標題的段；讀到段落後的「全集」就停）
pub fn video_meta(text: &str) -> Vec<VideoMeta> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut out: Vec<VideoMeta> = Vec::new();
    let mut cur: Option<(usize, Option<String>, Vec<String>)> = None;
    let finish = |cur: &mut Option<(usize, Option<String>, Vec<String>)>, out: &mut Vec<VideoMeta>| {
        if let Some((number, Some(title), lines)) = cur.take() {
            out.push(build(number, &title, &lines));
        }
    };
    for line in text.lines() {
        let l = line.trim();
        if let Some(n) = segment_label(l) {
            finish(&mut cur, &mut out);
            cur = Some((n, None, Vec::new()));
            continue;
        }
        if l == "=======" || (l == "全集" && !out.is_empty() && cur.is_none()) {
            finish(&mut cur, &mut out);
            if l == "全集" {
                break;
            }
            continue;
        }
        let Some((_, title, lines)) = cur.as_mut() else { continue };
        match title {
            None if !l.is_empty() => *title = Some(l.to_string()),
            None => {}
            Some(_) => lines.push(l.to_string()),
        }
    }
    finish(&mut cur, &mut out);
    out
}

fn build(number: usize, title: &str, lines: &[String]) -> VideoMeta {
    let mut warnings = Vec::new();
    let mut title = clean(title);
    if title.chars().count() > TITLE_MAX {
        warnings.push(format!("標題超過 {TITLE_MAX} 字，已截掉後面"));
        title = title.chars().take(TITLE_MAX).collect();
    }
    // 「後臺關鍵字」區塊不進說明，裡面的關鍵字當標記
    let (lines, keywords) = match lines.iter().position(|l| is_keywords_heading(l)) {
        Some(h) => {
            let kw = lines[h + 1..].iter().take_while(|l| is_keywords_line(l)).flat_map(|l| keywords_in_line(l)).collect();
            (&lines[..h], kw)
        }
        None => (lines, lines.iter().flat_map(|l| hashtags(l)).collect()),
    };
    // 頭尾空行拿掉，中間的空行保留
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.is_empty()).map_or(start, |e| e + 1);
    let mut description = clean(&lines[start..end].join("\n"));
    if description.len() > DESCRIPTION_MAX_BYTES {
        warnings.push("說明超過 5000 位元組，已截掉後面".to_string());
        let mut cut = DESCRIPTION_MAX_BYTES;
        while !description.is_char_boundary(cut) {
            cut -= 1;
        }
        description.truncate(cut);
    }
    VideoMeta { number, title, description, keywords, warnings }
}

/// 使用者填的「標記」（Studio 後台那欄；逗號、頓號或換行分隔）→ 標記清單。
/// 照填的原樣送（`#新聞直播` 也保留），只去掉重複的，合計不超過 500 字
pub fn parse_tags(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut total = 0;
    for t in input.split([',', '，', '、', '\n']) {
        let t = clean(t.trim());
        if t.is_empty() || out.contains(&t) {
            continue;
        }
        let len = t.chars().count() + if t.contains(' ') { 2 } else { 0 } + usize::from(!out.is_empty());
        if total + len > TAGS_MAX {
            break;
        }
        total += len;
        out.push(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "20261026關鍵標題1-6\n\n串一 https://youtu.be/x\n\n全集 \n=======\n\n第一段\n\n標題一！【關鍵時刻】20261026-1張炤和\n\n標題一！\n標題二？\n#川普 #伊朗\n\n=======\n第三段\n\n標題三【關鍵時刻】20261026-3張炤和\n\n標題三\n\n=======\n全集\n\n(00:00:00)Part1：標題一！【關鍵時刻】20261026-1張炤和\n";

    #[test]
    fn backend_keywords_become_tags_not_description() {
        let doc = "第一段\n\n標題一！【關鍵時刻】20261025-1張炤和\n\n標題一！\n#川普 #馬斯克\n\n後臺關鍵字\n,川普 ,馬斯克 ,普丁\n\n=======\n";
        let m = video_meta(doc);
        assert_eq!(m[0].description, "標題一！\n#川普 #馬斯克");
        assert_eq!(m[0].keywords, vec!["川普", "馬斯克", "普丁"]);
        // 沒有後臺關鍵字區塊（舊文稿）：用 #關鍵字
        assert_eq!(video_meta(DOC)[0].keywords, vec!["川普", "伊朗"]);
    }

    #[test]
    fn segments_title_and_description() {
        let m = video_meta(DOC);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].number, 1);
        assert_eq!(m[0].title, "標題一！【關鍵時刻】20261026-1張炤和");
        assert_eq!(m[0].description, "標題一！\n標題二？\n#川普 #伊朗");
        assert_eq!(m[1].number, 3);
        assert_eq!(m[1].description, "標題三");
    }

    #[test]
    fn crlf_and_bom() {
        let m = video_meta(&format!("\u{feff}{}", DOC.replace('\n', "\r\n")));
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].description, "標題一！\n標題二？\n#川普 #伊朗");
    }

    #[test]
    fn angle_brackets_and_long_title() {
        let long = "長".repeat(120);
        let m = video_meta(&format!("第二段\n\n{long}<x>\n\na<b\n=======\n"));
        assert_eq!(m[0].title.chars().count(), TITLE_MAX);
        assert_eq!(m[0].description, "a＜b");
        assert_eq!(m[0].warnings.len(), 1);
    }

    #[test]
    fn tags() {
        assert_eq!(parse_tags("東森, #新聞直播，東森、 新聞 直播\n,"), vec!["東森", "#新聞直播", "新聞 直播"]);
        let many = (0..200).map(|i| format!("標籤{i}")).collect::<Vec<_>>().join(",");
        let t = parse_tags(&many);
        let total: usize = t.iter().map(|s| s.chars().count()).sum::<usize>() + t.len() - 1;
        assert!(total <= TAGS_MAX);
    }
}
