use crate::fullwidth::to_fullwidth;

/// 開場標記：下一行標題就是這段的段標題。
const OPENING_MARKER: &str = "開場";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedSegment {
    /// 段標題：有開場用開場那行，否則用第一條標題。沒有任何標題時為 None。
    pub title: Option<String>,
    /// 標題列表（不含開場行），已轉全形。
    pub lines: Vec<String>,
    /// 引號數量為單數的行（已轉全形的文字），交給呼叫端產生警告。
    pub odd_quote_lines: Vec<String>,
}

/// 解析一段貼上的原始稿：去掉「一段」標記、講者名字和空行，標題轉全形。
///
/// `max_speaker_chars`：幾個字以內、沒有標點的行視為講者名字。標題動輒三四十字，
/// 名字（國禎、律師）只有兩三個字，用長度就能分得很乾淨。
///
/// 段標題：`開場` 下一行；沒寫 `開場` 但第一個講者前面有標題，那行也視為開場
/// （常見貼法是少貼了「開場」兩個字）；都沒有才用第一條標題。
pub fn parse_segment(raw: &str, max_speaker_chars: usize) -> ParsedSegment {
    let mut opening: Option<String> = None;
    let mut expect_opening = false;
    let mut seen_speaker = false;
    // 第一個講者出現前、且沒有「開場」標記時的標題
    let mut before_speaker = Vec::new();
    let mut lines = Vec::new();
    let mut odd_quote_lines = Vec::new();

    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if is_section_marker(line) {
            continue;
        }
        if line == OPENING_MARKER {
            expect_opening = true;
            continue;
        }
        if is_speaker(line, max_speaker_chars) {
            seen_speaker = true;
            expect_opening = false;
            continue;
        }

        let converted = to_fullwidth(line);
        if converted.odd_quotes {
            odd_quote_lines.push(converted.text.clone());
        }
        if expect_opening && opening.is_none() {
            opening = Some(converted.text);
            expect_opening = false;
        } else if !seen_speaker && opening.is_none() {
            before_speaker.push(converted.text);
        } else {
            lines.push(converted.text);
        }
    }

    // 整段都沒有講者名字時，前面那些就只是一般標題，不當開場。
    if seen_speaker && opening.is_none() && !before_speaker.is_empty() {
        opening = Some(before_speaker.remove(0));
    }
    before_speaker.append(&mut lines);
    let lines = before_speaker;

    let title = opening.or_else(|| lines.first().cloned());
    ParsedSegment {
        title,
        lines,
        odd_quote_lines,
    }
}

/// 「一段」「二段」「第三段」這類節次標記。
fn is_section_marker(line: &str) -> bool {
    let body = line.strip_prefix('第').unwrap_or(line);
    let Some(num) = body.strip_suffix('段') else {
        return false;
    };
    !num.is_empty() && num.chars().all(|c| "一二三四五六七八九十".contains(c) || c.is_ascii_digit())
}

fn is_speaker(line: &str, max_chars: usize) -> bool {
    line.chars().count() <= max_chars
        && !line
            .chars()
            .any(|c| c.is_ascii_punctuation() || "！？：；，、。「」".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"一段
開場
Intel"逆轉王牌"EUV廠爆炸!? 馬斯克投奔台積電..魏哲家嗆:食物不留給別人!

國禎
Intel"逆轉王牌"EUV廠爆炸!? 馬斯克投奔台積電..魏哲家嗆:食物不留給別人!
Intel挑戰台積電"最先進工廠"自爆!? EUV關鍵氣體"三氟化氮"炸翻1死2命危!?


政峰
DeepSeek大當機5小時"算力耗盡"!? 進入10月"密集故障"數小時罕見當機連發!
"#;

    #[test]
    fn opening_becomes_title_and_is_not_listed() {
        let p = parse_segment(SAMPLE, 6);
        assert_eq!(
            p.title.as_deref(),
            Some("Intel「逆轉王牌」EUV廠爆炸！？ 馬斯克投奔台積電..魏哲家嗆：食物不留給別人！")
        );
        assert_eq!(p.lines.len(), 3);
        assert_eq!(p.lines[0], p.title.clone().unwrap());
        assert!(p.lines[2].starts_with("DeepSeek大當機"));
    }

    #[test]
    fn without_opening_first_line_is_title() {
        let p = parse_segment("二段\n政峰\n徐佳青周遊列國花多少錢?\n\n律師\n割頸案家屬捐500萬!", 6);
        assert_eq!(p.title.as_deref(), Some("徐佳青周遊列國花多少錢？"));
        assert_eq!(p.lines, vec!["徐佳青周遊列國花多少錢？", "割頸案家屬捐500萬！"]);
    }

    #[test]
    fn line_before_first_speaker_counts_as_opening() {
        // 少貼了「開場」：第一行就是開場標題，不該在列表裡重複出現
        let p = parse_segment("開場標題很長很長很長!\n\n國禎\n開場標題很長很長很長!\n第二條標題很長很長", 6);
        assert_eq!(p.title.as_deref(), Some("開場標題很長很長很長！"));
        assert_eq!(p.lines, vec!["開場標題很長很長很長！", "第二條標題很長很長"]);
    }

    #[test]
    fn no_speakers_at_all_keeps_every_line() {
        let p = parse_segment("第一條標題很長很長\n第二條標題很長很長", 6);
        assert_eq!(p.title.as_deref(), Some("第一條標題很長很長"));
        assert_eq!(p.lines, vec!["第一條標題很長很長", "第二條標題很長很長"]);
    }

    #[test]
    fn only_names_means_no_title() {
        let p = parse_segment("三段\n敬平\n", 6);
        assert_eq!(p.title, None);
        assert!(p.lines.is_empty());
    }

    #[test]
    fn section_markers() {
        for m in ["一段", "二段", "第三段", "十段", "1段"] {
            assert!(is_section_marker(m), "{m}");
        }
        for m in ["段", "一段落", "手段"] {
            assert!(!is_section_marker(m), "{m}");
        }
    }

    #[test]
    fn short_title_with_punctuation_is_not_a_speaker() {
        assert!(!is_speaker("爆了!", 6));
        assert!(is_speaker("國禎", 6));
        assert!(is_speaker("律師", 6));
    }

    #[test]
    fn odd_quotes_reported() {
        let p = parse_segment("國禎\n開頭\"沒收尾的標題很長很長很長", 6);
        assert_eq!(p.odd_quote_lines.len(), 1);
    }
}
