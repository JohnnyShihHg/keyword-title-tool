//! 共用資料夾同步：讀寫當天資料夾裡的 txt（路徑規則在 core 的 `sync`）。
//!
//! 寫入做兩段驗證：先寫成同資料夾的暫存檔再改名蓋過正式檔（寫到一半斷線也不會留下
//! 半個檔案），改名後再讀回來逐字比對。覆蓋前比對內容，檔案和上次讀到／寫入的不一樣
//! （被別人改過）就回報 conflict，由使用者決定要載入對方的還是用自己的蓋掉。
//! 修改時間只拿來給畫面便宜地輪詢（網路磁碟的時間有快取，不拿來判斷衝突）。

use keyword_title_core::import::{self, ImportedDocument};
use keyword_title_core::render::to_crlf;
use keyword_title_core::{sync, Settings};
use serde::Serialize;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Serialize)]
pub struct SyncError {
    /// unreachable（連不上根目錄）／permission（沒有權限）／nofolder（當天資料夾還沒建立）
    /// ／nofile（txt 還沒建立，要使用者自己按寫入）／conflict（別人改過）／verify（讀回來不一致）／other
    kind: &'static str,
    message: String,
}

fn err(kind: &'static str, message: impl Into<String>) -> SyncError {
    SyncError { kind, message: message.into() }
}

fn io_err(e: std::io::Error, what: &str) -> SyncError {
    match e.kind() {
        ErrorKind::PermissionDenied => err("permission", format!("沒有權限{what}：{e}")),
        _ => err("other", format!("{what}失敗：{e}")),
    }
}

#[derive(Debug, Serialize)]
pub struct SyncFile {
    /// 當天資料夾在不在（工具不會自己建資料夾）
    pub folder: bool,
    pub exists: bool,
    /// 檔案原文（CRLF，去掉 BOM）
    pub text: String,
    /// 修改時間（毫秒）；不存在時 0
    pub mtime: u64,
}

#[derive(Debug, Serialize)]
pub struct Written {
    pub mtime: u64,
    /// 實際寫進去的內容（CRLF），之後拿來比對別人有沒有改
    pub text: String,
}

fn mtime(path: &Path) -> Result<Option<u64>, SyncError> {
    match std::fs::metadata(path) {
        Ok(m) => Ok(Some(
            m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64),
        )),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(e, "讀取檔案資訊")),
    }
}

/// `根目錄\當天資料夾\檔名` 的根目錄連得到嗎（網路斷線、VPN 沒連時連不到）
fn check_root(path: &Path) -> Result<(), SyncError> {
    let root = path.parent().and_then(Path::parent).ok_or_else(|| err("other", "路徑不完整"))?;
    match std::fs::metadata(root) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(err("unreachable", format!("{} 不是資料夾", root.display()))),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => Err(err("permission", format!("沒有權限開啟 {}", root.display()))),
        Err(_) => Err(err("unreachable", format!("連不上 {}", root.display()))),
    }
}

fn read_text(path: &Path) -> Result<Option<String>, SyncError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| err("other", "共用資料夾裡的 txt 不是 UTF-8 文字檔"))?;
            Ok(Some(text.trim_start_matches('\u{feff}').to_string()))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(e, "讀取")),
    }
}

pub fn read(path: &Path) -> Result<SyncFile, SyncError> {
    check_root(path)?;
    let folder = path.parent().is_some_and(Path::is_dir);
    match read_text(path)? {
        Some(text) => Ok(SyncFile { folder, exists: true, text, mtime: mtime(path)?.unwrap_or(0) }),
        None => Ok(SyncFile { folder, exists: false, text: String::new(), mtime: 0 }),
    }
}

/// known：上次讀到／寫入的內容（None＝以為檔案還不存在）。force：不管別人改過，直接蓋掉。
/// create：txt 還不存在時建立（只有使用者按「寫入」才建，自動同步不建）。資料夾一律不建。
pub fn write(path: &Path, content: &str, known: Option<&str>, force: bool, create: bool) -> Result<Written, SyncError> {
    check_root(path)?;
    let dir = path.parent().ok_or_else(|| err("other", "路徑不完整"))?;
    if !dir.is_dir() {
        return Err(err("nofolder", format!("當天資料夾 {} 還沒建立", dir.display())));
    }
    if !create && !path.exists() {
        return Err(err("nofile", "共用資料夾裡還沒有今天的 txt"));
    }

    let text = to_crlf(content);
    if !force {
        if let Some(current) = read_text(path)? {
            // 內容剛好和要寫的一樣（例如兩個人貼了同一批）就不用寫
            if current == text {
                return Ok(Written { mtime: mtime(path)?.unwrap_or(0), text });
            }
            if known != Some(current.as_str()) {
                return Err(err("conflict", "共用資料夾的檔案被別人改過"));
            }
        }
    }

    // 第一段：寫暫存檔再改名蓋過去
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp: PathBuf = dir.join(format!(".~{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| io_err(e, "寫入"))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // 有些共用資料夾不允許改名蓋檔，退回直接覆寫
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(path, text.as_bytes()).map_err(|_| io_err(e, "寫入"))?;
    }

    // 第二段：讀回來比對
    if read_text(path)?.as_deref() != Some(text.as_str()) {
        return Err(err("verify", "寫入後讀回來的內容不一致，請檢查網路或手動存一份"));
    }
    Ok(Written { mtime: mtime(path)?.unwrap_or(0), text })
}

// --- 指令 ---

#[tauri::command]
pub fn sync_path(settings: Settings, date: String) -> Option<String> {
    sync::file_path(&settings, &date).map(|p| p.to_string_lossy().into_owned())
}

// 共用資料夾是網路磁碟，可能讀很久：async 讓它在背景執行緒跑，不會卡住整個視窗。
#[tauri::command(async)]
pub fn sync_read(path: String) -> Result<SyncFile, SyncError> {
    read(Path::new(&path))
}

#[tauri::command(async)]
pub fn sync_stat(path: String) -> Result<Option<u64>, SyncError> {
    check_root(Path::new(&path))?;
    mtime(Path::new(&path))
}

#[tauri::command(async)]
pub fn sync_write(path: String, content: String, known: Option<String>, force: bool, create: bool) -> Result<Written, SyncError> {
    write(Path::new(&path), &content, known.as_deref(), force, create)
}

/// 共用資料夾的 txt 拆回六段。和「匯入 txt」不同，空檔（第一個人剛建的）也照樣回傳。
#[tauri::command]
pub fn import_text(text: String, settings: Settings) -> ImportedDocument {
    import::import(&text, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kt-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn never_creates_folder_and_creates_file_only_when_asked() {
        let r = root("flow");
        let path = r.join("1016關鍵").join("20261016關鍵標題1-6.txt");
        let f = read(&path).unwrap();
        assert!(!f.folder && !f.exists);
        assert_eq!(write(&path, "a", None, false, true).unwrap_err().kind, "nofolder");
        assert!(!path.parent().unwrap().exists(), "folder is never created");

        std::fs::create_dir(path.parent().unwrap()).unwrap();
        assert!(read(&path).unwrap().folder);
        assert_eq!(write(&path, "a", None, false, false).unwrap_err().kind, "nofile");
        let w = write(&path, "甲\n乙", None, false, true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "甲\r\n乙");
        let f = read(&path).unwrap();
        assert_eq!((f.exists, f.text.as_str(), f.mtime), (true, "甲\r\n乙", w.mtime));

        // 自己接著改：沒問題
        let w2 = write(&path, "甲\n乙\n丙", Some(&w.text), false, false).unwrap();
        // 不知道已經有檔案（第一次開、以為沒有）→ conflict
        assert_eq!(write(&path, "我的", None, false, true).unwrap_err().kind, "conflict");
        // 別人在中間改過 → conflict；force 才蓋
        std::fs::write(&path, "別人的").unwrap();
        assert_eq!(write(&path, "我的", Some(&w2.text), false, true).unwrap_err().kind, "conflict");
        write(&path, "我的", Some(&w2.text), true, true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "我的");
        // 不知道檔案已存在（expected None）但內容一樣：不算衝突
        assert!(write(&path, "我的", None, false, true).is_ok());
        assert!(std::fs::read_dir(path.parent().unwrap()).unwrap().count() == 1, "temp file cleaned up");
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn missing_root_is_unreachable() {
        let path = std::env::temp_dir().join("kt-sync-nope-root").join("1016關鍵").join("x.txt");
        assert_eq!(read(&path).unwrap_err().kind, "unreachable");
        assert_eq!(write(&path, "a", None, false, true).unwrap_err().kind, "unreachable");
    }

    #[test]
    fn bom_is_stripped() {
        let r = root("bom");
        let path = r.join("d").join("x.txt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "\u{feff}甲").unwrap();
        assert_eq!(read(&path).unwrap().text, "甲");
        let _ = std::fs::remove_dir_all(&r);
    }
}
