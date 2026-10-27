//! 文稿解析＋OCR 文字對段落。

use std::collections::BTreeMap;

/// 每段的標題清單，每行標題存成「兩字一組」
pub type Titles = BTreeMap<usize, Vec<Vec<(char, char)>>>;

/// 只留中英數字，全形轉半形
pub fn norm(s: &str) -> Vec<char> {
    s.chars()
        .map(|c| if ('！'..='～').contains(&c) { char::from_u32(c as u32 - 0xFEE0).unwrap_or(c) } else { c })
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

pub fn bigrams(v: &[char]) -> Vec<(char, char)> {
    v.windows(2).map(|w| (w[0], w[1])).collect()
}

/// 文稿 → 每段的標題清單（第一段、第二段…；遇到「全集」就停）
///
/// 段落標記支援「第一段」「第1段」與單獨一行的「1.」；遇到「=======」分隔線該段結束
/// （下一段的 YT 標題常寫在分隔線和段落標記之間）。全集章節時間是全集做完才有的，不讀。
pub fn parse_titles(text: &str) -> Titles {
    let num = |s: &str| -> Option<usize> {
        let s = s.trim();
        if let Some(t) = s.strip_prefix('第').and_then(|t| t.strip_suffix('段')) {
            return "一二三四五六七八九".find(t).map(|i| i / 3 + 1).or_else(|| t.parse().ok());
        }
        s.strip_suffix('.').or_else(|| s.strip_suffix('．')).and_then(|t| t.parse().ok()).filter(|n| (1..=9).contains(n))
    };
    let mut out: Titles = BTreeMap::new();
    let mut cur: Option<usize> = None;
    // 檔頭也有一行「全集」（YT 連結還沒填時只剩「全集 」），讀過段落後的「全集」才是結尾
    let mut seen_segment = false;
    for line in text.lines() {
        let l = line.trim().trim_start_matches('\u{feff}');
        if l == "全集" && seen_segment {
            break;
        }
        if let Some(n) = num(l) {
            cur = Some(n);
            seen_segment = true;
            continue;
        }
        if l.starts_with('=') {
            cur = None;
            continue;
        }
        if l.is_empty() || l.starts_with('#') || l.contains("http") || crate::parse::is_keywords_heading(l) || crate::parse::is_keywords_line(l) {
            continue;
        }
        if let Some(n) = cur {
            let main = l.split('【').next().unwrap_or(l);
            let b = bigrams(&norm(main));
            if b.len() >= 6 {
                out.entry(n).or_default().push(b);
            }
        }
    }
    out
}

/// OCR 文字屬於哪一段：OCR 的字組（兩字一組）有多少比例出現在某段的某一行標題裡（≥60%）
pub fn which_segment(ocr: &str, titles: &Titles) -> Option<(usize, f32)> {
    let a = bigrams(&norm(ocr));
    if a.len() < 6 {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    for (n, lines) in titles {
        for t in lines {
            let hit = a.iter().filter(|g| t.contains(g)).count() as f32 / a.len() as f32;
            if best.is_none_or(|b| hit > b.1) {
                best = Some((*n, hit));
            }
        }
    }
    best.filter(|b| b.1 >= 0.6)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOL_FORMAT: &str = "\u{feff}第一段\n\n川普關稅大棒再揮向盟友【關鍵時刻】20261022-1張炤和\n\n日本首相急訪白宮談判破局\n韓國半導體出口雪上加霜\n=======\n川普YT標題不該算進第一段的文字\n第二段\n\n伊朗忍者部隊潛入以色列境內\nhttps://youtu.be/xxxx\n短\n=======\n全集\n\n(00:00:00)Part1：川普關稅大棒再揮向盟友\n";

    #[test]
    fn parses_tool_output_format() {
        let t = parse_titles(TOOL_FORMAT);
        assert_eq!(t.keys().copied().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(t[&1].len(), 3, "分隔線後的 YT 標題不算");
        assert_eq!(t[&2].len(), 1, "網址、太短的行不算");
        // 「【」後面的節目名稱不算
        assert!(!t[&1][0].contains(&('關', '鍵')));
    }

    #[test]
    fn parses_numbered_format_and_stops_at_full_episode() {
        let t = parse_titles("1.\n中國辦公大樓大量空置\n2．\n重慶南方國際大樓空置危機\n第3段\n上海夜景大樓空置問題\n全集\n第四段\n不該讀到的第四段標題文字\n");
        assert_eq!(t.keys().copied().collect::<Vec<_>>(), [1, 2, 3]);
        assert!(parse_titles("10.\n超過九的編號不是段落標記喔\n").is_empty());
    }

    #[test]
    fn header_without_links_is_not_the_end() {
        // 直播當下檔頭的 YT 連結都還沒填：「全集 」trim 後就是「全集」，不能當成文稿結尾
        let t = parse_titles("20261026關鍵標題1-6\r\n\r\n串一 \r\n串二 \r\n\r\n全集 \r\n=======\r\n\r\n第一段\r\n\r\n最狠選舉操盤手川普預告轟伊朗逼烏俄和談\r\n=======\r\n第二段\r\n\r\n徐佳青涉貪卻獲邀僑委晚宴高歌逐桌敬酒\r\n=======\r\n全集\r\n\r\n(00:00:00)Part1：不該讀到的全集章節標題文字\r\n");
        assert_eq!(t.keys().copied().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(t[&2].len(), 1, "結尾的全集章節不算");
    }

    #[test]
    fn norm_halfwidth_lower_and_strip_punct() {
        assert_eq!(norm("ＡＢｃ１２！ 中文，Ok"), "abc12中文ok".chars().collect::<Vec<_>>());
    }

    #[test]
    fn ocr_text_matches_segment() {
        let t = parse_titles(TOOL_FORMAT);
        // OCR 常漏字、多雜訊，只要 60% 字組對得上就算
        assert_eq!(which_segment("日本首相急訪白宮談判破局", &t).map(|x| x.0), Some(1));
        assert_eq!(which_segment("伊朗忍者部隊潛入以色列境內", &t).map(|x| x.0), Some(2));
        assert_eq!(which_segment("伊朗忍者部隊潛入以色到境乃", &t).map(|x| x.0), Some(2));
        assert_eq!(which_segment("完全無關的廣告文字內容在這裡", &t), None);
        assert_eq!(which_segment("太短", &t), None);
    }
}
