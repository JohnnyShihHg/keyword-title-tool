#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod video;

use keyword_title_core::render::{self, DocumentInput, Rendered};
use keyword_title_core::Settings;
use std::path::PathBuf;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
fn render_document(doc: DocumentInput) -> Rendered {
    render::render(&doc)
}

/// 另存成 txt。內容一律轉成 CRLF，跟範本（Windows 記事本）一致。
#[tauri::command]
async fn save_text_file(app: tauri::AppHandle, date: String, content: String) -> Result<bool, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_file_name(render::file_name(&date))
        .add_filter("Text", &["txt"])
        .save_file(move |path| {
            let _ = tx.send(path);
        });
    match rx.recv().map_err(|e| e.to_string())? {
        Some(p) => {
            let path = p.into_path().map_err(|e| e.to_string())?;
            std::fs::write(path, render::to_crlf(&content)).map_err(|e| e.to_string())?;
            Ok(true)
        }
        None => Ok(false),
    }
}

// --- 設定後台：存在使用者設定資料夾的 settings.json ---

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|d| d.join("settings.json"))
        .map_err(|e| e.to_string())
}

/// 讀不到或壞掉就用預設值，不讓設定檔問題擋住主要功能。
#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> Settings {
    settings_path(&app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn default_settings() -> Settings {
    Settings::default()
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: Settings) -> Result<(), String> {
    let path = settings_path(&app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(video::VideoState::default())
        .invoke_handler(tauri::generate_handler![
            render_document,
            save_text_file,
            get_settings,
            default_settings,
            save_settings,
            video::tools_status,
            video::install_tools,
            video::probe_video,
            video::video_thumbnails,
            video::detect_encoder,
            video::download_youtube,
            video::render_video,
            video::cancel_render,
            video::output_path,
            video::path_exists,
            video::parse_timecode
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
