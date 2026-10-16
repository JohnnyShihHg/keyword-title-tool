//! 影片處理（預留）。
//!
//! 之後在這裡用 ffmpeg 做：
//! - 段落之間加閃白轉場
//! - 疊浮水印圖檔
//!
//! 預計做法：ffmpeg 以 sidecar 隨安裝檔附上（或在設定指定路徑），這裡負責組參數、
//! 用 `std::process::Command` 執行，並透過 Tauri event 把進度回傳給「影片」分頁。
//! 純參數組裝邏輯到時候放進 core（`core/src/video.rs`），才能單元測試。

use serde::Serialize;

#[derive(Serialize)]
pub struct VideoStatus {
    pub available: bool,
    pub message: String,
}

#[tauri::command]
pub fn video_status() -> VideoStatus {
    VideoStatus {
        available: false,
        message: "影片處理（閃白轉場、浮水印）尚未實作".to_string(),
    }
}
