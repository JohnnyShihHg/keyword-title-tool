#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autocut;
mod sync;
mod video;

use keyword_title_core::render::{self, DocumentInput, Rendered};
use keyword_title_core::Settings;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
fn render_document(doc: DocumentInput) -> Rendered {
    render::render(&doc)
}

/// 匯入之前匯出的 txt，拆回六段、時間、YT 連結和日期。
#[tauri::command(async)]
fn import_text_file(path: String, settings: keyword_title_core::settings::Settings) -> Result<keyword_title_core::import::ImportedDocument, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("讀不到檔案：{e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "這個檔案不是 UTF-8 文字檔（請匯入本工具「存成 txt」的檔案）".to_string())?;
    let doc = keyword_title_core::import::import(&text, &settings);
    if doc.segments.iter().all(|s| s.text.is_empty()) {
        return Err("這個檔案裡找不到任何一段標題，請確認是本工具匯出的 txt".into());
    }
    Ok(doc)
}

/// 一次貼上：整批文稿依「一段」「二段」…拆開（放進哪幾段由畫面決定）。
#[tauri::command]
fn split_batch(text: String) -> Vec<String> {
    keyword_title_core::batch::split_batch(&text)
}

/// 另存成 txt。內容一律轉成 CRLF，跟範本（Windows 記事本）一致。
/// dir：預設開在哪個資料夾（共用資料夾的當天資料夾），回傳存到的路徑。
#[tauri::command]
async fn save_text_file(app: tauri::AppHandle, date: String, content: String, dir: Option<String>) -> Result<Option<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut dialog = app.dialog().file().set_file_name(render::file_name(&date)).add_filter("Text", &["txt"]);
    if let Some(dir) = dir.filter(|d| std::path::Path::new(d).is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file(move |path| {
        let _ = tx.send(path);
    });
    match rx.recv().map_err(|e| e.to_string())? {
        Some(p) => {
            let path = p.into_path().map_err(|e| e.to_string())?;
            std::fs::write(&path, render::to_crlf(&content)).map_err(|e| e.to_string())?;
            Ok(Some(path.to_string_lossy().into_owned()))
        }
        None => Ok(None),
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

/// 關視窗前讓畫面先把還沒寫回共用資料夾的修改寫完（見 app.js 的 app-close-requested）。
static CLOSING: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn finish_close(app: tauri::AppHandle) {
    CLOSING.store(true, Ordering::SeqCst);
    app.exit(0);
}

/// 隱藏的開發用參數：`keyword-title-tool.exe --autocut-batch <設定.json>`
/// 不開畫面，照設定（同 autocut_start 的 AutocutConfig）把資料夾裡的側錄整批跑完、出完片就結束。
/// 紀錄寫在 `{成品資料夾}\.autocut\batch.log`，結果在 `{成品資料夾}\report.txt`。
fn batch_config() -> Option<Result<autocut::AutocutConfig, String>> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == "--autocut-batch")?;
    Some((|| {
        let path = args.get(i + 1).ok_or("--autocut-batch 後面要接設定檔路徑")?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("讀不到設定檔 {path}：{e}"))?;
        let mut cfg: autocut::AutocutConfig = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("設定檔格式錯誤：{e}"))?;
        cfg.batch = true;
        Ok(cfg)
    })())
}

fn main() {
    let batch = match batch_config() {
        Some(Ok(cfg)) => Some(cfg),
        Some(Err(e)) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
        None => None,
    };
    let mut builder = tauri::Builder::default();
    // 整批模式不註冊單一執行個體：工具開著時也能另外跑一個整批
    if batch.is_none() {
        // 必須第一個註冊：再次開啟程式時不開第二個，而是把已開的視窗叫到最前面
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    }
    builder
        .setup(move |app| {
            if let Some(cfg) = batch.clone() {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
                let handle = app.handle().clone();
                let state = app.state::<autocut::AutocutState>();
                if let Err(e) = autocut::start(handle, &state, cfg) {
                    eprintln!("{e}");
                    std::process::exit(2);
                }
            }
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(video::VideoState::default())
        .manage(autocut::AutocutState::default())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if CLOSING.swap(true, Ordering::SeqCst) {
                    return;
                }
                api.prevent_close();
                let _ = window.emit("app-close-requested", ());
                // 畫面沒回應（卡住或出錯）也不能讓視窗關不掉：最多等 5 秒
                let app = window.app_handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    app.exit(0);
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            render_document,
            import_text_file,
            split_batch,
            save_text_file,
            get_settings,
            default_settings,
            save_settings,
            finish_close,
            sync::sync_path,
            sync::sync_read,
            sync::sync_stat,
            sync::sync_write,
            sync::import_text,
            video::tools_status,
            video::install_tools,
            video::probe_video,
            video::video_thumbnails,
            video::detect_encoder,
            video::download_youtube,
            video::render_video,
            video::cancel_render,
            video::plan_video,
            video::output_path,
            video::path_exists,
            video::parse_timecode,
            video::detect_flash,
            autocut::autocut_start,
            autocut::autocut_stop,
            autocut::autocut_status,
            autocut::autocut_render,
            autocut::autocut_report,
            autocut::autocut_set_titles,
            autocut::autocut_set_chain,
            autocut::autocut_reveal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
