//! YouTube 上傳：Google 授權（開瀏覽器登入一次，之後用 refresh token）、可續傳上傳成私人影片。
//!
//! 標題、說明從文稿取（core 的 `youtube::video_meta`），標記由使用者每段自己填。
//! 只上傳成「私人」，片尾、中插、版權確認、發布都由人在 Studio 做。

use keyword_title_core::settings::Settings;
use keyword_title_core::youtube::{self, VideoMeta};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

/// `youtube` 涵蓋上傳，之後加播放清單也不用重新授權
const SCOPE: &str = "https://www.googleapis.com/auth/youtube";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const UPLOAD_URL: &str = "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status";
/// 每次送 16 MB（必須是 256 KB 的倍數）
const CHUNK: u64 = 16 * 1024 * 1024;
/// 「新聞與政治」
const CATEGORY_NEWS: &str = "25";

#[derive(Default)]
pub struct YoutubeState {
    /// 段號 → 取消旗標（上傳中才有）
    cancels: Mutex<HashMap<usize, Arc<AtomicBool>>>,
    /// 等瀏覽器登入中的那一次；再按登入就取消它重開（Google 擋下時不會回呼，不然要等 5 分鐘）
    login_cancel: Mutex<Option<Arc<AtomicBool>>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Token {
    refresh_token: String,
    access_token: String,
    /// UNIX 秒
    expires_at: u64,
    channel: String,
    /// Google 不再接受 refresh token（同意畫面在「測試」狀態約 7 天就失效，或被撤銷）：要重新登入
    #[serde(default)]
    expired: bool,
}

/// 登入過期時的錯誤訊息；畫面靠開頭這幾個字認出來（youtube.js 的 YT_EXPIRED）
pub const EXPIRED: &str = "YouTube 登入已過期";

#[derive(Clone, Serialize)]
struct UploadEvent {
    n: usize,
    path: String,
    /// uploading／done／failed／canceled
    state: &'static str,
    progress: f64,
    video_id: Option<String>,
    error: Option<String>,
    /// 影片傳好了，但縮圖或播放清單沒成功
    warning: Option<String>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn config_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(name))
}

fn load_token(app: &AppHandle) -> Option<Token> {
    let s = std::fs::read_to_string(config_file(app, "youtube_token.json").ok()?).ok()?;
    serde_json::from_str::<Token>(&s).ok().filter(|t| !t.refresh_token.is_empty())
}

fn save_token(app: &AppHandle, t: &Token) -> Result<(), String> {
    let json = serde_json::to_string_pretty(t).map_err(|e| e.to_string())?;
    std::fs::write(config_file(app, "youtube_token.json")?, json).map_err(|e| e.to_string())
}

/// 上傳過的檔：`路徑|大小` → 影片 ID（同一個檔不重複上傳）
fn load_uploads(app: &AppHandle) -> BTreeMap<String, String> {
    config_file(app, "youtube_uploads.json")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn upload_key(path: &str) -> Option<String> {
    let len = std::fs::metadata(path).ok()?.len();
    Some(format!("{path}|{len}"))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn random_state() -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    format!("{:016x}", h.finish())
}

fn check_client(settings: &Settings) -> Result<(), String> {
    if settings.yt_client_id.trim().is_empty() || settings.yt_client_secret.trim().is_empty() {
        return Err("請先到設定頁填 YouTube 的 OAuth 用戶端 ID 和密碼".into());
    }
    Ok(())
}

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v["error"]["message"].as_str().or(v["error_description"].as_str()).or(v["error"].as_str()).map(str::to_string)
                })
                .unwrap_or(body);
            format!("HTTP {code}：{msg}")
        }
        e => e.to_string(),
    }
}

fn json(r: ureq::Response) -> Result<serde_json::Value, String> {
    let body = r.into_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| format!("YouTube 回應看不懂：{e}"))
}

fn token_request(form: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    json(ureq::post(TOKEN_URL).timeout(Duration::from_secs(30)).send_form(form).map_err(http_err)?)
}

/// 有效的 access token（快過期就用 refresh token 換新的）
fn access_token(app: &AppHandle, settings: &Settings) -> Result<String, String> {
    let mut t = load_token(app).ok_or("還沒登入 YouTube")?;
    if t.expired {
        return Err(format!("{EXPIRED}，請按「重新登入」"));
    }
    if t.expires_at > now() + 120 && !t.access_token.is_empty() {
        return Ok(t.access_token);
    }
    let v = match token_request(&[
        ("client_id", settings.yt_client_id.trim()),
        ("client_secret", settings.yt_client_secret.trim()),
        ("refresh_token", &t.refresh_token),
        ("grant_type", "refresh_token"),
    ]) {
        Ok(v) => v,
        // 400／401＝Google 說這個 refresh token 不能用了（網路斷線不算，那是別的錯誤）
        Err(e) if e.starts_with("HTTP 400") || e.starts_with("HTTP 401") => {
            t.expired = true;
            t.access_token.clear();
            save_token(app, &t)?;
            return Err(format!("{EXPIRED}，請按「重新登入」（{e}）"));
        }
        Err(e) => return Err(format!("換 YouTube 授權失敗：{e}")),
    };
    t.access_token = v["access_token"].as_str().unwrap_or_default().to_string();
    t.expires_at = now() + v["expires_in"].as_u64().unwrap_or(3600);
    save_token(app, &t)?;
    Ok(t.access_token)
}

fn channel_title(token: &str) -> Result<String, String> {
    let v = json(
        ureq::get("https://www.googleapis.com/youtube/v3/channels?part=snippet&mine=true")
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(Duration::from_secs(30))
            .call()
            .map_err(http_err)?,
    )?;
    Ok(v["items"][0]["snippet"]["title"].as_str().unwrap_or("（這個帳號沒有頻道）").to_string())
}

/// 開瀏覽器登入 Google，等本機回呼拿到授權碼，換成 refresh token 存起來。回傳頻道名稱。
fn login(app: &AppHandle, settings: &Settings, cancel: &AtomicBool) -> Result<String, String> {
    check_client(settings)?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr().map_err(|e| e.to_string())?.port());
    let state = random_state();
    let url = format!(
        // select_account：每次都讓使用者選帳號，品牌帳戶（例如「關鍵時刻」頻道）才選得到、也能換帳號
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent%20select_account&state={state}",
        enc(settings.yt_client_id.trim()),
        enc(&redirect),
        enc(SCOPE)
    );
    std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &url]).spawn().map_err(|e| format!("開不了瀏覽器：{e}"))?;

    // 等瀏覽器回來（最多 5 分鐘）；其他請求（favicon 之類）略過
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(300);
    let params = loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("CANCELED".into());
        }
        if Instant::now() > deadline {
            return Err("等 Google 登入逾時（5 分鐘），請再按一次登入".into());
        }
        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let _ = stream.set_nonblocking(false);
        let mut line = String::new();
        let _ = BufReader::new(&stream).read_line(&mut line);
        let target = line.split_whitespace().nth(1).unwrap_or("");
        let query = target.split_once('?').map_or("", |q| q.1);
        let params: HashMap<String, String> = query.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), dec(v))).collect();
        if !params.contains_key("code") && !params.contains_key("error") {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }
        let ok = params.contains_key("code") && params.get("state") == Some(&state);
        let body = if ok { "授權完成，可以關掉這個分頁回到關鍵標題工具。" } else { "授權沒有完成，請回到關鍵標題工具再試一次。" };
        let page = format!("<!doctype html><meta charset=\"utf-8\"><title>關鍵標題工具</title><p style=\"font:18px sans-serif;margin:40px\">{body}</p>");
        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
        break params;
    };
    if let Some(e) = params.get("error") {
        return Err(format!("Google 沒有授權：{e}"));
    }
    if params.get("state") != Some(&state) {
        return Err("授權回應對不上（state 不符），請再試一次".into());
    }
    let code = params.get("code").cloned().unwrap_or_default();
    let v = token_request(&[
        ("code", &code),
        ("client_id", settings.yt_client_id.trim()),
        ("client_secret", settings.yt_client_secret.trim()),
        ("redirect_uri", &redirect),
        ("grant_type", "authorization_code"),
    ])?;
    let refresh = v["refresh_token"].as_str().unwrap_or_default().to_string();
    if refresh.is_empty() {
        return Err("Google 沒有給 refresh token，請到 Google 帳號的第三方存取移除本工具後再登入".into());
    }
    let access = v["access_token"].as_str().unwrap_or_default().to_string();
    let channel = channel_title(&access)?;
    save_token(
        app,
        &Token { refresh_token: refresh, access_token: access, expires_at: now() + v["expires_in"].as_u64().unwrap_or(3600), channel: channel.clone(), expired: false },
    )?;
    Ok(channel)
}

#[derive(Serialize)]
pub struct AccountStatus {
    logged_in: bool,
    channel: String,
    /// 登入過，但 Google 已經不接受了（要重新登入）
    expired: bool,
}

#[tauri::command]
pub fn yt_account(app: AppHandle) -> AccountStatus {
    match load_token(&app) {
        Some(t) => AccountStatus { logged_in: !t.expired, channel: t.channel, expired: t.expired },
        None => AccountStatus { logged_in: false, channel: String::new(), expired: false },
    }
}

#[tauri::command]
pub async fn yt_login(app: AppHandle, settings: Settings) -> Result<String, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(prev) = app.state::<YoutubeState>().login_cancel.lock().unwrap().replace(cancel.clone()) {
        prev.store(true, Ordering::SeqCst);
    }
    tauri::async_runtime::spawn_blocking(move || login(&app, &settings, &cancel)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn yt_logout(app: AppHandle) -> Result<(), String> {
    let p = config_file(&app, "youtube_token.json")?;
    if p.exists() {
        std::fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 文稿 → 每段的標題、說明
#[tauri::command]
pub fn yt_meta(text: String) -> Vec<VideoMeta> {
    youtube::video_meta(&text)
}

/// 這些檔上傳過的影片 ID（沒傳過的不列）
#[tauri::command]
pub fn yt_uploaded(app: AppHandle, paths: Vec<String>) -> HashMap<String, String> {
    let done = load_uploads(&app);
    paths.into_iter().filter_map(|p| Some((p.clone(), done.get(&upload_key(&p)?)?.clone()))).collect()
}

#[derive(Serialize)]
pub struct VideoState {
    /// private／unlisted／public；影片被刪掉＝None
    privacy: Option<String>,
    /// 排程公開時間（有排程才有）
    publish_at: Option<String>,
}

/// 這些影片在 YouTube 上現在的狀態（每 50 支一次查詢，1 個配額單位）
#[tauri::command]
pub async fn yt_video_states(app: AppHandle, settings: Settings, ids: Vec<String>) -> Result<HashMap<String, VideoState>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let token = access_token(&app, &settings)?;
        let mut out: HashMap<String, VideoState> = ids.iter().map(|id| (id.clone(), VideoState { privacy: None, publish_at: None })).collect();
        for chunk in ids.chunks(50) {
            let v = json(
                ureq::get(&format!("https://www.googleapis.com/youtube/v3/videos?part=status&id={}", enc(&chunk.join(","))))
                    .set("Authorization", &format!("Bearer {token}"))
                    .timeout(Duration::from_secs(30))
                    .call()
                    .map_err(http_err)?,
            )?;
            for item in v["items"].as_array().into_iter().flatten() {
                let id = item["id"].as_str().unwrap_or_default().to_string();
                out.insert(
                    id,
                    VideoState {
                        privacy: item["status"]["privacyStatus"].as_str().map(str::to_string),
                        publish_at: item["status"]["publishAt"].as_str().map(str::to_string),
                    },
                );
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 在瀏覽器開 Studio 的影片編輯頁（補片尾、中插、確認版權、發布）
#[tauri::command]
pub fn yt_open(video_id: String) -> Result<(), String> {
    if !video_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("影片 ID 不對".into());
    }
    let url = format!("https://studio.youtube.com/video/{video_id}/edit");
    std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &url]).spawn().map(|_| ()).map_err(|e| format!("開不了瀏覽器：{e}"))
}

#[tauri::command]
pub fn yt_cancel(state: State<YoutubeState>, n: usize) {
    if let Some(c) = state.cancels.lock().unwrap().get(&n) {
        c.store(true, Ordering::SeqCst);
    }
}

#[derive(Deserialize)]
pub struct UploadRequest {
    n: usize,
    path: String,
    title: String,
    description: String,
    /// 標記（預設標記＋文稿的後臺關鍵字，逗號分隔）
    tags: String,
    /// 自訂縮圖（共用資料夾的 `N.jpg`）；沒有就不設
    thumbnail: Option<String>,
    /// 上傳後加入的播放清單（可以多個）
    #[serde(default)]
    playlist_ids: Vec<String>,
}

/// 縮圖上限 2 MB（YouTube 規定）
const THUMB_MAX: u64 = 2 * 1024 * 1024;

fn set_thumbnail(token: &str, video_id: &str, path: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("讀不到縮圖：{e}"))?;
    if bytes.len() as u64 > THUMB_MAX {
        return Err(format!("縮圖 {:.1} MB，超過 YouTube 的 2 MB 上限", bytes.len() as f64 / 1048576.0));
    }
    let mime = if path.to_lowercase().ends_with(".png") { "image/png" } else { "image/jpeg" };
    ureq::post(&format!("https://www.googleapis.com/upload/youtube/v3/thumbnails/set?videoId={video_id}&uploadType=media"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", mime)
        .timeout(Duration::from_secs(120))
        .send_bytes(&bytes)
        .map_err(http_err)?;
    Ok(())
}

fn add_to_playlist(token: &str, video_id: &str, playlist_id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "snippet": { "playlistId": playlist_id, "resourceId": { "kind": "youtube#video", "videoId": video_id } } });
    ureq::post("https://www.googleapis.com/youtube/v3/playlistItems?part=snippet")
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json; charset=UTF-8")
        .timeout(Duration::from_secs(30))
        .send_string(&body.to_string())
        .map_err(http_err)?;
    Ok(())
}

/// 影片傳好後：設縮圖、加播放清單。失敗不算上傳失敗，回傳要提醒的事
fn finish(app: &AppHandle, settings: &Settings, video_id: &str, thumbnail: Option<&str>, playlist_ids: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();
    let token = match access_token(app, settings) {
        Ok(t) => t,
        Err(e) => return vec![e],
    };
    if let Some(t) = thumbnail.filter(|t| !t.is_empty()) {
        // 剛傳完的影片偶爾還查不到（404），等一下再試
        let mut result = set_thumbnail(&token, video_id, t);
        for _ in 0..3 {
            match &result {
                Err(e) if e.starts_with("HTTP 404") => {
                    std::thread::sleep(Duration::from_secs(10));
                    result = set_thumbnail(&token, video_id, t);
                }
                _ => break,
            }
        }
        if let Err(e) = result {
            warnings.push(format!("縮圖沒設成功：{e}"));
        }
    }
    for p in playlist_ids.iter().filter(|p| !p.is_empty()) {
        if let Err(e) = add_to_playlist(&token, video_id, p) {
            warnings.push(format!("沒加進播放清單（{p}）：{e}"));
        }
    }
    warnings
}

#[derive(Serialize)]
pub struct Playlist {
    id: String,
    title: String,
}

/// 頻道的播放清單（給下拉選單）
#[tauri::command]
pub async fn yt_playlists(app: AppHandle, settings: Settings) -> Result<Vec<Playlist>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let token = access_token(&app, &settings)?;
        let mut out = Vec::new();
        let mut page = String::new();
        loop {
            let v = json(
                ureq::get(&format!("https://www.googleapis.com/youtube/v3/playlists?part=snippet&mine=true&maxResults=50&pageToken={}", enc(&page)))
                    .set("Authorization", &format!("Bearer {token}"))
                    .timeout(Duration::from_secs(30))
                    .call()
                    .map_err(http_err)?,
            )?;
            for item in v["items"].as_array().into_iter().flatten() {
                out.push(Playlist { id: item["id"].as_str().unwrap_or_default().to_string(), title: item["snippet"]["title"].as_str().unwrap_or_default().to_string() });
            }
            match v["nextPageToken"].as_str() {
                Some(t) if out.len() < 500 => page = t.to_string(),
                _ => break,
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn yt_upload(app: AppHandle, state: State<YoutubeState>, settings: Settings, req: UploadRequest) -> Result<(), String> {
    check_client(&settings)?;
    if load_token(&app).ok_or("還沒登入 YouTube")?.expired {
        return Err(format!("{EXPIRED}，請按「重新登入」"));
    }
    if !Path::new(&req.path).is_file() {
        return Err(format!("找不到檔案：{}", req.path));
    }
    if req.title.trim().is_empty() {
        return Err(format!("文稿裡沒有第 {} 段的標題", req.n));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut c = state.cancels.lock().unwrap();
        if c.contains_key(&req.n) {
            return Err(format!("第 {} 段已經在上傳", req.n));
        }
        c.insert(req.n, cancel.clone());
    }
    let app2 = app.clone();
    std::thread::spawn(move || {
        let (n, path) = (req.n, req.path.clone());
        let emit = |state: &'static str, progress: f64, video_id: Option<String>, error: Option<String>, warning: Option<String>| {
            let _ = app2.emit("yt-upload", UploadEvent { n, path: path.clone(), state, progress, video_id, error, warning });
        };
        emit("uploading", 0.0, None, None, None);
        match upload(&app2, &settings, &req, &cancel, &|p| emit("uploading", p, None, None, None)) {
            Ok(id) => {
                if let Some(key) = upload_key(&path) {
                    let mut done = load_uploads(&app2);
                    done.insert(key, id.clone());
                    if let (Ok(p), Ok(json)) = (config_file(&app2, "youtube_uploads.json"), serde_json::to_string_pretty(&done)) {
                        let _ = std::fs::write(p, json);
                    }
                }
                let warnings = finish(&app2, &settings, &id, req.thumbnail.as_deref(), &req.playlist_ids);
                emit("done", 1.0, Some(id), None, (!warnings.is_empty()).then(|| warnings.join("；")));
            }
            Err(_) if cancel.load(Ordering::SeqCst) => emit("canceled", 0.0, None, None, None),
            Err(e) => emit("failed", 0.0, None, Some(e), None),
        }
        app2.state::<YoutubeState>().cancels.lock().unwrap().remove(&n);
    });
    Ok(())
}

/// 可續傳上傳：先開一個上傳工作階段，再一塊一塊送；斷線就問伺服器收到哪裡、從那裡接著送。
fn upload(app: &AppHandle, settings: &Settings, req: &UploadRequest, cancel: &AtomicBool, progress: &dyn Fn(f64)) -> Result<String, String> {
    let mut file = std::fs::File::open(&req.path).map_err(|e| e.to_string())?;
    let total = file.metadata().map_err(|e| e.to_string())?.len();
    let tags = youtube::parse_tags(&req.tags);
    // 說明加上固定內容後可能超過 5000 位元組
    let mut description = req.description.clone();
    if description.len() > youtube::DESCRIPTION_MAX_BYTES {
        let mut cut = youtube::DESCRIPTION_MAX_BYTES;
        while !description.is_char_boundary(cut) {
            cut -= 1;
        }
        description.truncate(cut);
    }
    let body = serde_json::json!({
        "snippet": {
            "title": req.title,
            "description": description,
            "tags": tags,
            "categoryId": CATEGORY_NEWS,
            "defaultLanguage": "zh-TW",
            "defaultAudioLanguage": "zh-TW",
        },
        "status": { "privacyStatus": "private", "selfDeclaredMadeForKids": false },
    });
    let agent = ureq::AgentBuilder::new().redirects(0).timeout_connect(Duration::from_secs(30)).timeout_read(Duration::from_secs(300)).timeout_write(Duration::from_secs(300)).build();

    let session = agent
        .post(UPLOAD_URL)
        .set("Authorization", &format!("Bearer {}", access_token(app, settings)?))
        .set("Content-Type", "application/json; charset=UTF-8")
        .set("X-Upload-Content-Type", "video/mp4")
        .set("X-Upload-Content-Length", &total.to_string())
        .send_string(&body.to_string())
        .map_err(http_err)?
        .header("Location")
        .ok_or("YouTube 沒有回傳上傳位置")?
        .to_string();

    let mut offset = 0u64;
    let mut failures = 0u32;
    let mut buf = Vec::with_capacity(CHUNK as usize);
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("已取消".into());
        }
        let end = (offset + CHUNK).min(total);
        buf.clear();
        file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        (&mut file).take(end - offset).read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let token = access_token(app, settings)?;
        let resp = agent
            .put(&session)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "video/mp4")
            .set("Content-Range", &format!("bytes {offset}-{}/{total}", end - 1))
            .send_bytes(&buf);
        let next = match resp {
            Ok(r) if r.status() == 200 || r.status() == 201 => {
                let v: serde_json::Value = json(r)?;
                progress(1.0);
                return v["id"].as_str().map(str::to_string).ok_or_else(|| "YouTube 沒有回傳影片 ID".to_string());
            }
            Ok(r) if r.status() == 308 => Ok(received(&r)),
            Ok(r) => Err(format!("HTTP {}", r.status())),
            Err(ureq::Error::Status(404 | 410, _)) => return Err("上傳工作階段過期，請重新上傳".into()),
            Err(ureq::Error::Status(code, r)) if code < 500 && code != 401 && code != 408 && code != 429 => return Err(http_err(ureq::Error::Status(code, r))),
            Err(e) => Err(http_err(e)),
        };
        match next {
            Ok(n) => {
                offset = n;
                failures = 0;
                progress(offset as f64 / total as f64);
            }
            Err(e) => {
                failures += 1;
                if failures > 8 {
                    return Err(format!("上傳一直失敗：{e}"));
                }
                std::thread::sleep(Duration::from_secs(2u64.pow(failures.min(6))));
                // 問伺服器實際收到哪裡
                let token = access_token(app, settings)?;
                if let Ok(r) = agent.put(&session).set("Authorization", &format!("Bearer {token}")).set("Content-Range", &format!("bytes */{total}")).send_bytes(&[]) {
                    match r.status() {
                        200 | 201 => {
                            let v: serde_json::Value = json(r)?;
                            return v["id"].as_str().map(str::to_string).ok_or_else(|| "YouTube 沒有回傳影片 ID".to_string());
                        }
                        308 => offset = received(&r),
                        _ => {}
                    }
                }
            }
        }
    }
}

/// 308 回應的 `Range: bytes=0-12345` → 下一個要送的位置（沒有 Range＝什麼都還沒收到）
fn received(r: &ureq::Response) -> u64 {
    r.header("Range").and_then(|v| v.rsplit('-').next()).and_then(|n| n.trim().parse::<u64>().ok()).map_or(0, |n| n + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encoding_round_trip() {
        let s = "4/0AbC-d_e.f~g h+i/j?k=l&m";
        assert_eq!(dec(&enc(s)), s);
        assert_eq!(enc("http://127.0.0.1:5000"), "http%3A%2F%2F127.0.0.1%3A5000");
        assert_eq!(dec("4%2F0Ab"), "4/0Ab");
    }
}
