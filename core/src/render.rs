use crate::parse::parse_segment;
use crate::settings::Settings;
use crate::timecode::{self, Timecode};
use serde::{Deserialize, Serialize};

pub const SEGMENT_COUNT: usize = 6;
const DIVIDER: &str = "=======";
const SEGMENT_NAMES: [&str; SEGMENT_COUNT] = ["一", "二", "三", "四", "五", "六"];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SegmentInput {
    pub text: String,
    pub time: String,
    /// 這段上傳後的 YouTube 連結，填在檔頭「串一」後面。
    #[serde(default)]
    pub link: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DocumentInput {
    /// `20261014`
    pub date: String,
    pub segments: Vec<SegmentInput>,
    /// 全集的 YouTube 連結，填在檔頭「全集」後面。
    #[serde(default)]
    pub full_link: String,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Warning {
    /// 1 起算的段號；None 表示整份文件的問題。
    pub segment: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rendered {
    /// 換行為 `\n`；存檔時再轉 CRLF（見 [`to_crlf`]）。
    pub text: String,
    pub warnings: Vec<Warning>,
    /// 每段解析出的時間，回填輸入框用（`00:27:08`）；空白或格式錯誤為 None。
    pub times: Vec<Option<String>>,
    /// 有內容的段落與它的 YouTube 標題行，給影片頁的檔名下拉選單用。
    pub parts: Vec<PartSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PartSummary {
    pub number: usize,
    /// `段標題【關鍵時刻】20261016-1張炤和`
    pub title_line: String,
}

/// 輸出檔名：`20261014關鍵標題1-6.txt`
pub fn file_name(date: &str) -> String {
    format!("{}關鍵標題1-{SEGMENT_COUNT}.txt", date.trim())
}

/// 範本是 Windows 記事本存的，換行為 CRLF。
pub fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

struct Part {
    number: usize,
    title: String,
    lines: Vec<String>,
    time: Option<Timecode>,
}

/// 依範本排出整份文稿。空的段落直接略過——稿子是分批進來的，
/// 沒填的段就是還沒拿到，不算錯誤。
pub fn render(doc: &DocumentInput) -> Rendered {
    let date = doc.date.trim();
    let mut warnings = Vec::new();
    let mut parts = Vec::new();
    let mut times = Vec::new();

    if date.len() != 8 || !date.chars().all(|c| c.is_ascii_digit()) {
        warnings.push(Warning {
            segment: None,
            message: format!("日期應為 8 位數字（例如 20261014），目前是「{date}」"),
        });
    }

    for (i, seg) in doc.segments.iter().take(SEGMENT_COUNT).enumerate() {
        let number = i + 1;
        let warn = |message: String| Warning {
            segment: Some(number),
            message,
        };

        let time = match timecode::parse(&seg.time) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(warn(e));
                None
            }
        };
        times.push(time.map(Timecode::to_halfwidth));

        let parsed = parse_segment(&seg.text, doc.settings.max_speaker_chars);
        for line in &parsed.odd_quote_lines {
            warnings.push(warn(format!("引號數量為單數，請確認「」配對：{line}")));
        }
        let Some(title) = parsed.title else {
            continue;
        };

        // 第一段固定從片頭開始，沒填時間就當 00:00:00。
        let time = if time.is_none() && number == 1 && seg.time.trim().is_empty() {
            Some(Timecode { total_seconds: 0 })
        } else {
            time
        };
        if time.is_none() && seg.time.trim().is_empty() {
            warnings.push(warn("還沒填時間點".to_string()));
        }

        parts.push(Part {
            number,
            title,
            lines: parsed.lines,
            time,
        });
    }

    for pair in parts.windows(2) {
        if let (Some(a), Some(b)) = (pair[0].time, pair[1].time) {
            if b <= a {
                warnings.push(Warning {
                    segment: Some(pair[1].number),
                    message: format!(
                        "時間點 {} 沒有晚於第{}段的 {}",
                        b.to_halfwidth(),
                        pair[0].number,
                        a.to_halfwidth()
                    ),
                });
            }
        }
    }

    let summaries = parts
        .iter()
        .map(|p| PartSummary {
            number: p.number,
            title_line: title_line(p, date, &doc.settings),
        })
        .collect();

    Rendered {
        text: layout(date, &parts, doc),
        warnings,
        times,
        parts: summaries,
    }
}

fn title_line(p: &Part, date: &str, settings: &Settings) -> String {
    format!("{}{}{date}-{}{}", p.title, settings.program_tag, p.number, settings.host)
}

fn layout(date: &str, parts: &[Part], doc: &DocumentInput) -> String {
    let settings = &doc.settings;
    let Settings { host, program_tag: tag, .. } = settings;
    let mut out: Vec<String> = Vec::new();

    // 檔頭：日期標題、串一～串六、全集，後面接各自的 YouTube 連結（範本留一個空格給連結）。
    if settings.show_header {
        out.push(format!("{date}關鍵標題1-{SEGMENT_COUNT}"));
        out.push(String::new());
        for (i, name) in SEGMENT_NAMES.iter().enumerate() {
            let link = doc.segments.get(i).map_or("", |s| s.link.trim());
            out.push(format!("串{name} {link}"));
        }
        out.push(String::new());
        out.push(format!("全集 {}", doc.full_link.trim()));
        out.push(DIVIDER.to_string());
        out.push(String::new());
    }
    // 範本第一段開頭是 open；開啟段號標註時改成「第一段」，之後每段在 ======= 下一行接「第N段」。
    if !settings.label_segments {
        out.push("open".to_string());
    }

    for p in parts {
        if settings.label_segments {
            out.push(format!("第{}段", SEGMENT_NAMES[p.number - 1]));
        }
        out.push(String::new());
        out.push(title_line(p, date, settings));
        out.push(String::new());
        if settings.number_line {
            out.push(String::new());
            out.push(format!("{}.", p.number));
        }
        out.extend(p.lines.iter().cloned());
        out.push(String::new());
        out.push(DIVIDER.to_string());
    }

    out.push("全集".to_string());
    for p in parts {
        // YouTube 只認半形時間（00:27:08）才能點擊跳轉；前面緊貼全形括號也可能認不出來，
        // 所以括號一起用半形。關掉設定就回到範本的全形寫法。
        let time_part = if settings.halfwidth_timestamps {
            let time = p.time.map(Timecode::to_halfwidth).unwrap_or_else(|| "--:--:--".to_string());
            format!("({time})")
        } else {
            let time = p.time.map(Timecode::to_fullwidth).unwrap_or_else(|| "--：--：--".to_string());
            format!("（{time}）")
        };
        out.push(String::new());
        out.push(format!(
            // 半形「-」，和各段標題一致；全形「－」字身自帶左右空白，看起來像「 - 」
            "{time_part}Part{}：{}{tag}{date}-{}{host}",
            p.number, p.title, p.number
        ));
    }

    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, time: &str) -> SegmentInput {
        SegmentInput {
            text: text.into(),
            time: time.into(),
            link: String::new(),
        }
    }

    fn doc(segments: Vec<SegmentInput>) -> DocumentInput {
        DocumentInput {
            date: "20261016".into(),
            segments,
            full_link: String::new(),
            settings: Settings {
                show_header: false,
                ..Settings::default()
            },
        }
    }

    #[test]
    fn empty_segments_are_skipped_silently() {
        let r = render(&doc(vec![
            seg("開場\n標題一!\n國禎\n標題一!\n標題二?", ""),
            seg("", ""),
            seg("政峰\n第三段的標題很長很長", "0:44:59"),
        ]));
        assert!(r.text.contains("標題一！【關鍵時刻】20261016-1張炤和"));
        assert!(r.text.contains("=======\n第三段\n\n第三段的標題很長很長【關鍵時刻】"));
        assert!(!r.text.contains("第二段"));
        assert!(r.text.contains("(00:00:00)Part1：標題一！【關鍵時刻】20261016-1張炤和"));
        assert!(r.text.contains("(00:44:59)Part3：第三段的標題很長很長【關鍵時刻】20261016-3張炤和"));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn parts_list_title_lines_for_file_names() {
        let r = render(&doc(vec![seg("", ""), seg("乙標題乙標題乙", "1:00")]));
        assert_eq!(r.parts.len(), 1);
        assert_eq!(r.parts[0].number, 2);
        assert_eq!(r.parts[0].title_line, "乙標題乙標題乙【關鍵時刻】20261016-2張炤和");
    }

    #[test]
    fn missing_time_after_first_segment_warns_and_shows_placeholder() {
        let r = render(&doc(vec![seg("甲標題甲標題甲", ""), seg("乙標題乙標題乙", "")]));
        assert!(r.text.contains("(--:--:--)Part2："));
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.warnings[0].segment, Some(2));
    }

    #[test]
    fn non_increasing_time_warns() {
        let r = render(&doc(vec![
            seg("甲標題甲標題甲", "0"),
            seg("乙標題乙標題乙", "30:00"),
            seg("丙標題丙標題丙", "20:00"),
        ]));
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.warnings[0].segment, Some(3));
        assert_eq!(r.times[2].as_deref(), Some("00:20:00"));
    }

    #[test]
    fn bad_date_warns() {
        let mut d = doc(vec![]);
        d.date = "2026/10/16".into();
        assert_eq!(render(&d).warnings.len(), 1);
    }

    #[test]
    fn default_output_labels_each_segment_at_its_start() {
        let r = render(&doc(vec![seg("甲標題甲標題甲", ""), seg("乙標題乙標題乙", "1:00")]));
        assert!(r.text.starts_with(
            "第一段\n\n甲標題甲標題甲【關鍵時刻】20261016-1張炤和\n\n甲標題甲標題甲\n\n=======\n第二段\n\n乙標題"
        ));
        assert!(r.text.contains("張炤和\n\n乙標題乙標題乙\n\n=======\n全集\n"));
        assert!(!r.text.contains("1."));
        assert!(!r.text.contains("open"));
    }

    #[test]
    fn label_follows_segment_number_when_first_is_empty() {
        let r = render(&doc(vec![seg("", ""), seg("乙標題乙標題乙", "1:00")]));
        assert!(r.text.starts_with("第二段\n\n乙標題"));
    }

    #[test]
    fn settings_change_host_tag_and_layout() {
        let mut d = doc(vec![seg("甲標題甲標題甲", "")]);
        d.settings = Settings {
            host: "代班主持".into(),
            program_tag: "【別的節目】".into(),
            label_segments: false,
            number_line: true,
            show_header: true,
            ..Settings::default()
        };
        let r = render(&d);
        assert!(r.text.starts_with("20261016關鍵標題1-6\n"));
        assert!(r.text.contains("甲標題甲標題甲【別的節目】20261016-1代班主持"));
        assert!(r.text.contains("(00:00:00)Part1：甲標題甲標題甲【別的節目】20261016-1代班主持"));
        assert!(!r.text.contains("第一段"));
        assert!(r.text.contains("\nopen\n\n甲標題"));
        assert!(r.text.contains("代班主持\n\n\n1.\n甲標題"));
    }

    #[test]
    fn fullwidth_timestamps_when_setting_off() {
        let mut d = doc(vec![seg("甲標題甲標題甲", "1:02:03")]);
        d.settings.halfwidth_timestamps = false;
        assert!(render(&d).text.contains("（01：02：03）Part1："));
    }

    #[test]
    fn header_lists_youtube_links() {
        let mut d = doc(vec![seg("甲標題甲標題甲", ""), seg("乙標題乙標題乙", "1:00")]);
        d.settings.show_header = true;
        d.segments[0].link = " https://youtu.be/aaa ".into();
        d.full_link = "https://youtu.be/full".into();
        let r = render(&d);
        assert!(r.text.starts_with(
            "20261016關鍵標題1-6\n\n串一 https://youtu.be/aaa\n串二 \n串三 \n串四 \n串五 \n串六 \n\n全集 https://youtu.be/full\n=======\n\n第一段\n"
        ));
    }

    #[test]
    fn crlf_conversion_is_idempotent() {
        assert_eq!(to_crlf("a\nb\r\nc"), "a\r\nb\r\nc");
    }

    #[test]
    fn file_name_follows_template() {
        assert_eq!(file_name("20261014"), "20261014關鍵標題1-6.txt");
    }
}
