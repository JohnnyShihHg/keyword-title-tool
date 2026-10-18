//! 共用資料夾同步的路徑規則：`{根目錄}\{當天資料夾}\{日期}關鍵標題1-6.txt`。
//! 讀寫檔案在 app 端（src-tauri/src/sync.rs），這裡只算名字。

use crate::render::file_name;
use crate::settings::Settings;
use std::path::PathBuf;

/// 資料夾名稱格式代入日期（`20261016`）：`{YYYYMMDD}` `{YYYY}` `{MMDD}` `{MM}` `{DD}`。
/// 日期不是 8 位數字就回傳 None（還在打字或打錯，不要亂建資料夾）。
pub fn folder_name(format: &str, date: &str) -> Option<String> {
    let date = date.trim();
    if date.len() != 8 || !date.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (y, m, d) = (&date[..4], &date[4..6], &date[6..]);
    let name = format
        .replace("{YYYYMMDD}", date)
        .replace("{YYYY}", y)
        .replace("{MMDD}", &format!("{m}{d}"))
        .replace("{MM}", m)
        .replace("{DD}", d);
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// 當天 txt 的完整路徑；根目錄沒填或日期不完整時 None。
pub fn file_path(settings: &Settings, date: &str) -> Option<PathBuf> {
    let root = settings.sync_root.trim();
    if root.is_empty() {
        return None;
    }
    let folder = folder_name(&settings.sync_folder_format, date)?;
    Some(PathBuf::from(root).join(folder).join(file_name(date)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name_uses_month_and_day() {
        assert_eq!(folder_name("{MMDD}關鍵", "20261016").as_deref(), Some("1016關鍵"));
        assert_eq!(folder_name("{YYYY}/{MM}/{DD}", "20260105").as_deref(), Some("2026/01/05"));
        assert_eq!(folder_name("{MMDD}關鍵", "2026101"), None);
        assert_eq!(folder_name("{MMDD}關鍵", "2026101a"), None);
    }

    #[test]
    fn default_path() {
        let p = file_path(&Settings::default(), "20261016").unwrap();
        let s = p.to_string_lossy();
        assert!(s.starts_with(r"\\數媒yt上傳\yt上傳"), "{s}");
        assert!(s.contains("1016關鍵"));
        assert!(s.ends_with("20261016關鍵標題1-6.txt"));
    }
}
