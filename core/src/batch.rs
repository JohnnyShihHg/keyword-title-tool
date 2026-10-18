//! 「一次貼上」：把製作單位一次給的整批文稿（一行「一段」「二段」…分開）拆成一段一段。
//!
//! 每批（10 點、11 點各一次）都從「一段」開始編，所以這裡只負責切開，不看段號；
//! 要放進第幾段由 app 端決定（依序放進還空著的段落）。

/// 一行只有「一段」～「六段」（也接受「第一段」、「1段」、全形數字）就是新的一段開始。
fn is_marker(line: &str) -> bool {
    let line = line.trim();
    let line = line.strip_prefix('第').unwrap_or(line);
    let Some(num) = line.strip_suffix('段') else { return false };
    let mut chars = num.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return false };
    "一二三四五六七八九十123456789１２３４５６７８９".contains(c)
}

/// 拆成每段的原始文字（照貼上的順序，去掉段號那行和頭尾空行）。
/// 沒有任何段號行時整批當成一段；第一個段號行之前若有內容也算一段。
pub fn split_batch(text: &str) -> Vec<String> {
    let text = text.replace("\r\n", "\n");
    let mut parts: Vec<Vec<&str>> = vec![Vec::new()];
    for line in text.lines() {
        if is_marker(line) {
            parts.push(Vec::new());
        } else {
            parts.last_mut().unwrap().push(line.trim_end());
        }
    }
    parts
        .into_iter()
        .map(|lines| lines.join("\n").trim_matches('\n').to_string())
        .filter(|p| !p.trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_segment_lines() {
        let text = std::fs::read_to_string(format!("{}/tests/fixtures/batch/1016-10am.txt", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let parts = split_batch(&text);
        assert_eq!(parts.len(), 3);
        assert!(parts[0].starts_with("開場\n特斯拉"));
        assert!(parts[0].ends_with("AI系統全用上了!?"));
        assert!(parts[1].starts_with("家名\n運鈔車"));
        assert!(parts[2].starts_with("瑞德\n絕命終結站"));
        assert!(!parts.iter().any(|p| p.contains("二段\n") || p.starts_with("三段")));
    }

    #[test]
    fn marker_variants_and_crlf() {
        let parts = split_batch("第一段\r\n甲\r\n\r\n 2段 \r\n乙\r\n四段\r\n");
        assert_eq!(parts, ["甲", "乙"]);
    }

    #[test]
    fn titles_mentioning_segments_are_not_markers() {
        assert_eq!(split_batch("一段\n這一段是標題\n分段"), ["這一段是標題\n分段"]);
    }

    #[test]
    fn no_markers_is_one_segment() {
        assert_eq!(split_batch("\n開場\n甲\n"), ["開場\n甲"]);
        assert!(split_batch("  \n").is_empty());
    }
}
