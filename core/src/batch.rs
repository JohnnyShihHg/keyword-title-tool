//! 「一次貼上」：把製作單位一次給的整批文稿（一行「一段」「二段」…分開）拆成一段一段。
//!
//! 每批（10 點、11 點各一次）有的從「一段」開始編、有的照真正段號（「2-1」「3段」），
//! 所以這裡只負責切開，不看段號；要放進第幾段由 app 端決定（依序放進還空著的段落）。

/// 一個字的段號（一～十、1～9、全形）→ 數字。
fn seg_num(c: char) -> Option<u32> {
    ["一二三四五六七八九十", "123456789", "１２３４５６７８９"]
        .iter()
        .find_map(|set| set.chars().position(|d| d == c))
        .map(|i| i as u32 + 1)
}

/// 段號後面接的講者名字：短、沒有標點（像「家名」「厚之」）。
fn is_name(s: &str) -> bool {
    s.chars().count() <= 6 && s.chars().all(char::is_alphanumeric)
}

enum Marker<'a> {
    /// 「一段」「第一段」「1段」「3段 政峰」：一定是新的一段。
    Segment(&'a str),
    /// 「2-1 家名」「2-2厚之」：第 2 段的第幾位講者；段號跟上一個一樣就不切開。
    Part(u32, &'a str),
}

/// 段號行（後面可以接講者名字），回傳段號種類和名字（沒有名字＝空字串）。
fn marker(line: &str) -> Option<Marker<'_>> {
    let line = line.trim();
    let body = line.strip_prefix('第').unwrap_or(line);
    let mut chars = body.chars();
    let num = seg_num(chars.next()?)?;
    let after = chars.as_str();
    if let Some(rest) = after.strip_prefix('段') {
        // 名字前面要有空格，免得「一段時間…」這種標題被當成段號
        let ok = rest.is_empty() || (rest.starts_with(char::is_whitespace) && is_name(rest.trim()));
        return ok.then(|| Marker::Segment(rest.trim()));
    }
    // 「2-1」只接受阿拉伯數字、前面不加「第」
    if body.len() != line.len() || !line.starts_with(|c: char| c.is_ascii_digit() || ('１'..='９').contains(&c)) {
        return None;
    }
    let rest = after.strip_prefix(['-', '－', '–'])?;
    let mut rest_chars = rest.chars();
    rest_chars.next().filter(|c| c.is_ascii_digit() || ('１'..='９').contains(c))?;
    let name = rest_chars.as_str();
    (!name.starts_with(|c: char| c.is_ascii_digit()) && is_name(name.trim())).then(|| Marker::Part(num, name.trim()))
}

/// 拆成每段的原始文字（照貼上的順序，去掉段號、頭尾空行；段號後面的講者名字留成一行）。
/// 沒有任何段號行時整批當成一段；第一個段號行之前若有內容也算一段。
pub fn split_batch(text: &str) -> Vec<String> {
    let text = text.replace("\r\n", "\n");
    let mut parts: Vec<Vec<&str>> = vec![Vec::new()];
    let mut part_of: Option<u32> = None; // 上一個「N-M」的 N
    for line in text.lines() {
        let name = match marker(line) {
            Some(Marker::Segment(name)) => {
                part_of = None;
                name
            }
            Some(Marker::Part(num, name)) => {
                if part_of == Some(num) {
                    parts.last_mut().unwrap().push("");
                    parts.last_mut().unwrap().push(name);
                    continue;
                }
                part_of = Some(num);
                name
            }
            None => {
                parts.last_mut().unwrap().push(line.trim_end());
                continue;
            }
        };
        parts.push(Vec::new());
        if !name.is_empty() {
            parts.last_mut().unwrap().push(name);
        }
    }
    parts
        .into_iter()
        .map(|lines| {
            // 名字前補的空行跟原本的空行疊在一起時只留一行
            let mut out: Vec<&str> = Vec::new();
            for l in lines {
                if !(l.is_empty() && out.last().is_some_and(|p| p.is_empty())) {
                    out.push(l);
                }
            }
            out.join("\n").trim_matches('\n').to_string()
        })
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

    /// 11/19 第二批的寫法：「2-1 家名」「2-2厚之」「3段 政峰」
    #[test]
    fn numbered_markers_with_names() {
        let text = "2-1 家名\n\n標題甲!?\n標題乙\n\n2-2厚之\n\n標題丙\n \n2-3 惠珍\n標題丁\n\n3段 政峰\n標題戊\n";
        assert_eq!(
            split_batch(text),
            ["家名\n\n標題甲!?\n標題乙\n\n厚之\n\n標題丙\n\n惠珍\n標題丁", "政峰\n標題戊"]
        );
        // 段號變了就切開；名字前沒有空行的補一行
        assert_eq!(split_batch("1-1 甲\nA\n1-2 乙\nB\n2-1 丙\nC"), ["甲\nA\n\n乙\nB", "丙\nC"]);
    }

    #[test]
    fn titles_starting_like_markers_stay() {
        let text = "一段\n一段時間後\n2-1逆轉勝!?政院拍板\n3段 這是一條很長的標題!\n2-10 甲";
        assert_eq!(split_batch(text), ["一段時間後\n2-1逆轉勝!?政院拍板\n3段 這是一條很長的標題!\n2-10 甲"]);
    }

    /// 11/19 實際貼的兩批：第一批沒有段號（一段），第二批「2-1…」「3段 政峰」拆成兩段
    #[test]
    fn real_batches_1119() {
        let read = |f: &str| std::fs::read_to_string(format!("{}/tests/fixtures/batch/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let first = split_batch(&read("1119-10am.txt"));
        assert_eq!(first.len(), 1);
        assert!(first[0].starts_with("開場\n局勢大亂"));
        let second = split_batch(&read("1119-11am.txt"));
        assert_eq!(second.len(), 2);
        assert!(second[0].starts_with("家名\n\n選前倒數9天"));
        assert!(second[0].contains("\n\n厚之\n\n水庫光電") && second[0].contains("\n\n惠珍\n\"兩個姨\""));
        assert!(second[0].ends_with("亂象增!?"));
        assert!(second[1].starts_with("政峰\n新竹市"));
        assert!(second[1].ends_with("人氣爆棚!"));
    }
}
