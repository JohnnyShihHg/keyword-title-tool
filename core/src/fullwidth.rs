/// 標題的全形轉換結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Converted {
    pub text: String,
    /// 半形雙引號 `"` 數量為單數，「」配對可能不對，需要人工確認。
    pub odd_quotes: bool,
}

/// 依範本把一行標題轉成全形標點。
///
/// - `! ? : ; ( )` 轉成全形
/// - `,` 轉成 `，`，但夾在兩個數字中間（千分位）時保留
/// - 成對的 `"` 依序轉成「」；彎引號 `“”` 直接對應「」
/// - `.`、`..`、數字、英文、`+ - %`、空白一律保持原樣（範本如此）
pub fn to_fullwidth(line: &str) -> Converted {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len() + 8);
    let mut quote_open = false;
    let mut quote_count = 0usize;

    for (i, &c) in chars.iter().enumerate() {
        let mapped = match c {
            '!' => '！',
            '?' => '？',
            ':' => '：',
            ';' => '；',
            '(' => '（',
            ')' => '）',
            '“' => '「',
            '”' => '」',
            ',' => {
                let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
                let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
                if prev_digit && next_digit {
                    ','
                } else {
                    '，'
                }
            }
            '"' => {
                quote_count += 1;
                quote_open = !quote_open;
                if quote_open {
                    '「'
                } else {
                    '」'
                }
            }
            other => other,
        };
        out.push(mapped);
    }

    Converted {
        text: out,
        odd_quotes: quote_count % 2 != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_sample_title() {
        let r = to_fullwidth(r#"Intel"逆轉王牌"EUV廠爆炸!? 馬斯克投奔台積電..魏哲家嗆:食物不留給別人!"#);
        assert_eq!(
            r.text,
            "Intel「逆轉王牌」EUV廠爆炸！？ 馬斯克投奔台積電..魏哲家嗆：食物不留給別人！"
        );
        assert!(!r.odd_quotes);
    }

    #[test]
    fn keeps_dots_digits_and_latin() {
        let r = to_fullwidth("2奈米達營收8%.加3奈米共佔41% Intel殖利率6.92%..台積電才2%");
        assert_eq!(r.text, "2奈米達營收8%.加3奈米共佔41% Intel殖利率6.92%..台積電才2%");
    }

    #[test]
    fn quotes_alternate_and_can_start_a_line() {
        let r = to_fullwidth(r#""朝俄大橋"1年火速完工 林:金正恩戒備中國! 習"赴朝催生"大橋"#);
        assert_eq!(r.text, "「朝俄大橋」1年火速完工 林：金正恩戒備中國！ 習「赴朝催生」大橋");
    }

    #[test]
    fn odd_quote_count_is_flagged() {
        let r = to_fullwidth(r#"開頭"沒收尾"#);
        assert_eq!(r.text, "開頭「沒收尾");
        assert!(r.odd_quotes);
    }

    #[test]
    fn comma_kept_between_digits_only() {
        assert_eq!(to_fullwidth("砸1,000億,還不夠").text, "砸1,000億，還不夠");
    }

    #[test]
    fn already_fullwidth_is_unchanged() {
        let s = "定調中國敵對國家！ Anthropic封殺帳號.美銀撤資「SI挑戰賽」再補刀！？";
        assert_eq!(to_fullwidth(s).text, s);
    }
}
