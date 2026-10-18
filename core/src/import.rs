//! 把匯出的 txt（`render` 的成品）拆回文稿頁的輸入：日期、六段內容、時間、YT 連結。
//!
//! 匯出時講者名字行已經刪掉，匯入後也不會回來；但它們本來就不進成品，所以
//! 匯入 → 再匯出的結果和原檔一字不差（見測試）。每段還原成「開場＋段標題＋標題列表」，
//! 開場那行就是段標題，解析時不會被當成列表的一條。

use crate::render::SEGMENT_COUNT;
use crate::settings::Settings;
use serde::Serialize;

const DIVIDER: &str = "=======";
const SEGMENT_NAMES: [&str; SEGMENT_COUNT] = ["一", "二", "三", "四", "五", "六"];

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ImportedSegment {
    pub text: String,
    /// `00:27:08`；匯出檔裡是 `--:--:--` 或沒有就空白
    pub time: String,
    pub link: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ImportedDocument {
    /// 找不到就空白（畫面上保留原本的日期）
    pub date: String,
    pub full_link: String,
    /// 固定六段，沒有內容的段落 text 為空
    pub segments: Vec<ImportedSegment>,
}

fn fold_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            '：' => ':',
            '－' => '-',
            _ => c,
        })
        .collect()
}

fn segment_index(name: &str) -> Option<usize> {
    SEGMENT_NAMES.iter().position(|n| *n == name)
}

/// 段號：一～六或 1～6（含全形）
fn segment_number(c: char) -> Option<usize> {
    segment_index(&c.to_string()).or_else(|| {
        let d = fold_digits(&c.to_string()).chars().next()?.to_digit(10)? as usize;
        (1..=SEGMENT_COUNT).contains(&d).then(|| d - 1)
    })
}

fn strip_separator(s: &str) -> &str {
    s.trim().trim_start_matches([':', '：']).trim()
}

fn looks_like_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with("youtu") || s.starts_with("www.")
}

/// 檔頭的 `串一 https://…` → (段的索引, 連結)。同事手寫時會打成「拆一」、「串1」、「串一：」，都認。
fn header_link(line: &str) -> Option<(usize, &str)> {
    let rest = line.strip_prefix('串').or_else(|| line.strip_prefix('拆'))?;
    let mut chars = rest.chars();
    let i = segment_number(chars.next()?)?;
    Some((i, strip_separator(chars.as_str())))
}

/// `段標題【關鍵時刻】20261014-1張炤和` → (段標題, 日期, 段號)。全集行（`(…)Part1：`）不算。
fn split_title_line<'a>(line: &'a str, settings: &Settings) -> Option<(&'a str, String, usize)> {
    let tag = settings.program_tag.trim();
    let at = if tag.is_empty() { None } else { line.rfind(tag) }?;
    let title = &line[..at];
    let rest = fold_digits(&line[at + tag.len()..]);
    let (date, rest) = rest.split_once('-')?;
    if date.len() != 8 || !date.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let number = rest.chars().next()?.to_digit(10)? as usize;
    if !(1..=SEGMENT_COUNT).contains(&number) || title.trim().is_empty() {
        return None;
    }
    Some((title, date.to_string(), number))
}

/// 全集段的 `(00:27:08)Part3：…` 或 `（００：２７：０８）Part3：…` → (段號, 時間)
fn full_line_time(line: &str) -> Option<(usize, String)> {
    let line = fold_digits(line);
    let rest = line.strip_prefix('(').or_else(|| line.strip_prefix('（'))?;
    let close = rest.find([')', '）'])?;
    let time = rest[..close].trim().to_string();
    let after = rest[close..].trim_start_matches([')', '）']);
    let number = after.strip_prefix("Part")?.chars().next()?.to_digit(10)? as usize;
    if !(1..=SEGMENT_COUNT).contains(&number) {
        return None;
    }
    let valid = time.split(':').count() == 3 && time.chars().all(|c| c.is_ascii_digit() || c == ':');
    Some((number, if valid { time } else { String::new() }))
}

pub fn import(text: &str, settings: &Settings) -> ImportedDocument {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let mut doc = ImportedDocument { segments: vec![ImportedSegment::default(); SEGMENT_COUNT], ..Default::default() };

    // 檔頭：日期標題、串一～串六、全集（連結空白就留空）
    let mut body_start = 0;
    if let Some(first) = lines.iter().position(|l| !l.trim().is_empty()) {
        let head = lines[first].trim();
        if let Some(date) = head.split("關鍵標題").next().filter(|d| head.contains("關鍵標題") && d.len() == 8) {
            doc.date = date.to_string();
            let end = lines[first..].iter().position(|l| l.trim() == DIVIDER).map_or(first + 1, |p| first + p + 1);
            let header: Vec<&str> = lines[first + 1..end].iter().map(|l| l.trim()).collect();
            for (k, line) in header.iter().enumerate() {
                // 連結換到下一行貼的也算：下一行是網址、不是另一個串N／全集
                let next_url = header.get(k + 1).filter(|l| looks_like_url(l)).copied();
                if let Some((i, link)) = header_link(line) {
                    doc.segments[i].link = if link.is_empty() { next_url.unwrap_or("") } else { link }.to_string();
                } else if let Some(rest) = line.strip_prefix("全集") {
                    let link = strip_separator(rest);
                    doc.full_link = if link.is_empty() { next_url.unwrap_or("") } else { link }.to_string();
                }
            }
            body_start = end;
        }
    }

    // 內文：用 ======= 分成一塊一塊；最後一塊是「全集」時間表
    let mut blocks: Vec<Vec<&str>> = vec![Vec::new()];
    for line in &lines[body_start..] {
        if line.trim() == DIVIDER {
            blocks.push(Vec::new());
        } else {
            blocks.last_mut().unwrap().push(line.trim());
        }
    }

    let mut next_free = 0;
    for block in blocks {
        let content: Vec<&str> = block.into_iter().filter(|l| !l.is_empty()).collect();
        if content.first() == Some(&"全集") {
            for line in &content[1..] {
                if let Some((n, time)) = full_line_time(line) {
                    doc.segments[n - 1].time = time;
                }
            }
            continue;
        }
        // 段號標註（open／第N段）和「1.」編號行不是內容
        let content: Vec<&str> = content
            .into_iter()
            .filter(|l| {
                *l != "open"
                    && !(l.starts_with('第') && l.ends_with('段') && l.chars().count() == 3)
                    && !(l.ends_with('.') && l[..l.len() - 1].chars().all(|c| c.is_ascii_digit()) && l.len() > 1)
            })
            .collect();
        let Some(pos) = content.iter().position(|l| split_title_line(l, settings).is_some()) else {
            continue;
        };
        let (title, date, number) = split_title_line(content[pos], settings).unwrap();
        if doc.date.is_empty() {
            doc.date = date;
        }
        // 段號在標題行裡；已經被佔用（不該發生）就依序放到下一個空的段
        let mut index = number - 1;
        if !doc.segments[index].text.is_empty() {
            while next_free < SEGMENT_COUNT && !doc.segments[next_free].text.is_empty() {
                next_free += 1;
            }
            if next_free >= SEGMENT_COUNT {
                continue;
            }
            index = next_free;
        }
        let list = content[pos + 1..].join("\n");
        doc.segments[index].text = format!("開場\n{title}\n{list}").trim_end().to_string();
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{render, DocumentInput, SegmentInput};

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/20261014/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn original() -> DocumentInput {
        let times: Vec<String> = fixture("times.txt").lines().map(str::to_string).collect();
        DocumentInput {
            date: "20261014".into(),
            segments: (1..=6)
                .map(|n| SegmentInput {
                    text: fixture(&format!("seg{n}.txt")),
                    time: times[n - 1].clone(),
                    link: if n == 2 { "https://youtu.be/abc".into() } else { String::new() },
                })
                .collect(),
            full_link: "https://youtu.be/full".into(),
            settings: Settings::default(),
        }
    }

    fn back_to_input(doc: &ImportedDocument, settings: &Settings) -> DocumentInput {
        DocumentInput {
            date: doc.date.clone(),
            segments: doc
                .segments
                .iter()
                .map(|s| SegmentInput { text: s.text.clone(), time: s.time.clone(), link: s.link.clone() })
                .collect(),
            full_link: doc.full_link.clone(),
            settings: settings.clone(),
        }
    }

    #[test]
    fn export_then_import_then_export_is_identical() {
        let input = original();
        let exported = render(&input).text;
        let imported = import(&exported, &input.settings);
        assert_eq!(imported.date, "20261014");
        assert_eq!(imported.full_link, "https://youtu.be/full");
        assert_eq!(imported.segments[1].link, "https://youtu.be/abc");
        assert_eq!(imported.segments[0].link, "", "empty links stay empty");
        assert_eq!(imported.segments[1].time, "00:27:08");
        let again = render(&back_to_input(&imported, &input.settings)).text;
        assert_eq!(again, exported);
    }

    #[test]
    fn hand_written_header_links_are_recognised() {
        let input = original();
        let body = render(&input).text;
        let body = &body[body.find("=======").unwrap()..];
        let header = "20261014關鍵標題1-6\n\n拆一 https://youtu.be/a\n串2：https://youtu.be/b\n串三\nhttps://youtu.be/c\n串四 \n串五\n串六\n\n全集：https://youtu.be/full\n";
        let doc = import(&format!("{header}{body}"), &input.settings);
        let links: Vec<&str> = doc.segments.iter().map(|s| s.link.as_str()).collect();
        assert_eq!(links, ["https://youtu.be/a", "https://youtu.be/b", "https://youtu.be/c", "", "", ""]);
        assert_eq!(doc.full_link, "https://youtu.be/full");
    }

    #[test]
    fn crlf_file_with_bom_imports_the_same() {
        let input = original();
        let exported = render(&input).text;
        let file = format!("\u{feff}{}", crate::render::to_crlf(&exported));
        assert_eq!(import(&file, &input.settings), import(&exported, &input.settings));
    }

    #[test]
    fn missing_segments_stay_empty_and_others_keep_their_numbers() {
        let mut input = original();
        input.segments[1].text.clear();
        input.segments[4].text.clear();
        let exported = render(&input).text;
        let imported = import(&exported, &input.settings);
        assert!(imported.segments[1].text.is_empty());
        assert!(imported.segments[4].text.is_empty());
        assert!(!imported.segments[2].text.is_empty());
        assert_eq!(render(&back_to_input(&imported, &input.settings)).text, exported);
    }

    #[test]
    fn other_layout_settings_round_trip_too() {
        // 範本原樣：open、1. 編號行、全形時間、沒有檔頭
        let mut input = original();
        input.settings.label_segments = false;
        input.settings.number_line = true;
        input.settings.halfwidth_timestamps = false;
        input.settings.show_header = false;
        let exported = render(&input).text;
        let imported = import(&exported, &input.settings);
        assert_eq!(imported.date, "20261014", "date comes from the title lines without a header");
        assert_eq!(imported.segments[2].time, "00:44:59");
        assert_eq!(render(&back_to_input(&imported, &input.settings)).text, exported);
    }
}
