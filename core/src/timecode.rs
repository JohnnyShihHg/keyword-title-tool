/// 影片時間點（YouTube 章節用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timecode {
    pub total_seconds: u32,
}

impl Timecode {
    pub fn hms(self) -> (u32, u32, u32) {
        let s = self.total_seconds;
        (s / 3600, s / 60 % 60, s % 60)
    }

    /// `00:27:08`，給輸入框回填用。
    pub fn to_halfwidth(self) -> String {
        let (h, m, s) = self.hms();
        format!("{h:02}:{m:02}:{s:02}")
    }

    /// `00：27：08`，範本全集區使用全形冒號。
    pub fn to_fullwidth(self) -> String {
        let (h, m, s) = self.hms();
        format!("{h:02}：{m:02}：{s:02}")
    }
}

/// 解析使用者輸入的時間。空白回傳 `Ok(None)`。
///
/// 接受：`0:27:08`、`27:08`（分:秒）、`2708`、`012708`（純數字從右往左補成 時分秒），
/// 全形數字和全形冒號也可以。
pub fn parse(input: &str) -> Result<Option<Timecode>, String> {
    let normalized: String = input
        .trim()
        .chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap(),
            '：' => ':',
            other => other,
        })
        .collect();

    if normalized.is_empty() {
        return Ok(None);
    }

    let invalid = || format!("時間格式看不懂：「{}」，請輸入像 00:27:08 或 2708", input.trim());

    let (h, m, s) = if normalized.contains(':') {
        let parts: Vec<&str> = normalized.split(':').collect();
        let nums: Vec<u32> = parts
            .iter()
            .map(|p| {
                if p.is_empty() || p.len() > 2 && parts.len() == 3 || !p.chars().all(|c| c.is_ascii_digit()) {
                    Err(())
                } else {
                    p.parse::<u32>().map_err(|_| ())
                }
            })
            .collect::<Result<_, _>>()
            .map_err(|_| invalid())?;
        match nums.as_slice() {
            [m, s] => (0, *m, *s),
            [h, m, s] => {
                if *m >= 60 {
                    return Err(format!("分鐘不能超過 59：「{}」", input.trim()));
                }
                (*h, *m, *s)
            }
            _ => return Err(invalid()),
        }
    } else {
        if normalized.len() > 6 || !normalized.chars().all(|c| c.is_ascii_digit()) {
            return Err(invalid());
        }
        let padded = format!("{normalized:0>6}");
        let n = |r: std::ops::Range<usize>| padded[r].parse::<u32>().unwrap();
        let (h, m, s) = (n(0..2), n(2..4), n(4..6));
        if m >= 60 {
            return Err(format!("分鐘不能超過 59：「{}」", input.trim()));
        }
        (h, m, s)
    };

    if s >= 60 {
        return Err(format!("秒數不能超過 59：「{}」", input.trim()));
    }
    Ok(Some(Timecode {
        total_seconds: h * 3600 + m * 60 + s,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> String {
        parse(s).unwrap().unwrap().to_halfwidth()
    }

    #[test]
    fn accepted_forms() {
        assert_eq!(p("0:27:08"), "00:27:08");
        assert_eq!(p("00:27:08"), "00:27:08");
        assert_eq!(p("27:08"), "00:27:08");
        assert_eq!(p("2708"), "00:27:08");
        assert_eq!(p("11723"), "01:17:23");
        assert_eq!(p("011723"), "01:17:23");
        assert_eq!(p("１：１７：２３"), "01:17:23");
        assert_eq!(p("0"), "00:00:00");
    }

    #[test]
    fn minutes_over_60_in_mm_ss_roll_into_hours() {
        assert_eq!(p("90:00"), "01:30:00");
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(parse("  ").unwrap(), None);
    }

    #[test]
    fn rejects_garbage_and_out_of_range() {
        assert!(parse("abc").is_err());
        assert!(parse("1:2:3:4").is_err());
        assert!(parse("1:60:00").is_err());
        assert!(parse("2760").is_err());
        assert!(parse("1234567").is_err());
        assert!(parse("12::30").is_err());
    }

    #[test]
    fn fullwidth_output() {
        assert_eq!(parse("1:30:16").unwrap().unwrap().to_fullwidth(), "01：30：16");
    }
}
