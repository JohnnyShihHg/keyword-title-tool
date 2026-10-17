//! 影片處理：工具下載（ffmpeg / ffprobe / yt-dlp）、讀影片資訊、縮圖、YouTube 下載、
//! 硬體編碼偵測、串接輸出。參數組裝和輸出解析都在 core（`keyword_title_core::video`），
//! 這裡只負責執行程式和回報進度。

use base64::Engine;
use keyword_title_core::video::{self, Encoder, ProbeInfo, RenderSpec, YtdlpLine};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub struct VideoState {
    render: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
    encoder: Mutex<Option<EncoderReport>>,
}

// --- 工具位置 ---

const FFMPEG: &str = "ffmpeg";
/// Windows 版 ffmpeg 鎖定 7.1.1：9.0 的 NVENC 要求 NVIDIA 驅動 610 以上，但 Pascal 世代
/// （Quadro P2000/P5000、GTX 10 系列）的驅動最新只到 580，用 9.0 就永遠用不了 N 卡編碼。
/// 換版本時改這裡，已安裝的會被提示重新下載。
const FFMPEG_BUILD: &str = "7.1.1-essentials";
const FFMPEG_BUILD_MARKER: &str = "ffmpeg-build.txt";
const FFPROBE: &str = "ffprobe";
const YTDLP: &str = "yt-dlp";

fn tools_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|d| d.join("tools"))
        .map_err(|e| e.to_string())
}

fn exe_name(tool: &str) -> String {
    format!("{tool}{}", std::env::consts::EXE_SUFFIX)
}

/// 先找程式自己下載的工具，找不到再找系統 PATH（開發時用 brew 裝的也能跑）。
fn find_tool(app: &AppHandle, tool: &str) -> Option<PathBuf> {
    let name = exe_name(tool);
    if let Ok(dir) = tools_dir(app) {
        let p = dir.join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(&name))
            .find(|p| p.is_file())
    })
}

fn require_tool(app: &AppHandle, tool: &str) -> Result<PathBuf, String> {
    find_tool(app, tool).ok_or_else(|| format!("找不到 {tool}，請到「設定 → 影片工具」按「下載工具」"))
}

/// 開子程序。Windows 上要加 CREATE_NO_WINDOW，否則每次呼叫 ffmpeg 都會閃一個黑色命令列視窗。
fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Serialize)]
pub struct ToolsStatus {
    ffmpeg: Option<String>,
    ffprobe: Option<String>,
    ytdlp: Option<String>,
    /// 這個平台能不能自動下載（Windows、Apple Silicon Mac）
    can_install: bool,
    /// 程式資料夾裡的 ffmpeg 不是目前指定的版本，建議重新下載
    ffmpeg_outdated: bool,
}

fn status(app: &AppHandle) -> ToolsStatus {
    let show = |t: &str| find_tool(app, t).map(|p| p.display().to_string());
    let ffmpeg_outdated = cfg!(windows)
        && tools_dir(app).is_ok_and(|dir| {
            dir.join(exe_name(FFMPEG)).is_file()
                && std::fs::read_to_string(dir.join(FFMPEG_BUILD_MARKER)).map_or(true, |v| v.trim() != FFMPEG_BUILD)
        });
    ToolsStatus {
        ffmpeg: show(FFMPEG),
        ffprobe: show(FFPROBE),
        ytdlp: show(YTDLP),
        can_install: !downloads_for(true, true).is_empty(),
        ffmpeg_outdated,
    }
}

#[tauri::command]
pub fn tools_status(app: AppHandle) -> ToolsStatus {
    status(&app)
}

// --- 工具下載 ---

enum Payload {
    /// 壓縮檔：(壓縮檔內路徑結尾, 存成的檔名)
    Zip(&'static [(&'static str, &'static str)]),
    /// 單一執行檔，存成這個檔名
    File(&'static str),
}

struct Download {
    label: &'static str,
    url: &'static str,
    payload: Payload,
}

fn downloads_for(ffmpeg: bool, ytdlp: bool) -> Vec<Download> {
    let mut list = Vec::new();
    if cfg!(windows) {
        if ffmpeg {
            list.push(Download {
                label: "ffmpeg",
                url: "https://github.com/GyanD/codexffmpeg/releases/download/7.1.1/ffmpeg-7.1.1-essentials_build.zip",
                payload: Payload::Zip(&[("bin/ffmpeg.exe", "ffmpeg.exe"), ("bin/ffprobe.exe", "ffprobe.exe")]),
            });
        }
        if ytdlp {
            list.push(Download {
                label: "yt-dlp",
                url: "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
                payload: Payload::File("yt-dlp.exe"),
            });
        }
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        if ffmpeg {
            list.push(Download {
                label: "ffmpeg",
                url: "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip",
                payload: Payload::Zip(&[("ffmpeg", "ffmpeg")]),
            });
            list.push(Download {
                label: "ffprobe",
                url: "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffprobe.zip",
                payload: Payload::Zip(&[("ffprobe", "ffprobe")]),
            });
        }
        if ytdlp {
            list.push(Download {
                label: "yt-dlp",
                url: "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos",
                payload: Payload::File("yt-dlp"),
            });
        }
    }
    list
}

#[derive(Clone, Serialize)]
struct ToolsProgress {
    label: String,
    downloaded: u64,
    total: u64,
}

/// `which`：`all`、`ffmpeg`、`ytdlp`。yt-dlp 要常更新（YouTube 常改版），所以可以單獨重抓。
#[tauri::command]
pub async fn install_tools(app: AppHandle, which: String) -> Result<ToolsStatus, String> {
    let (ffmpeg, ytdlp) = match which.as_str() {
        "ffmpeg" => (true, false),
        "ytdlp" => (false, true),
        _ => (true, true),
    };
    let list = downloads_for(ffmpeg, ytdlp);
    if list.is_empty() {
        return Err("這個系統不支援自動下載，請自行安裝 ffmpeg 和 yt-dlp".to_string());
    }
    let dir = tools_dir(&app)?;
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        install_into(&dir, &list, &|p| {
            let _ = app2.emit("tools-progress", p);
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(status(&app))
}

fn install_into(dir: &Path, list: &[Download], on_progress: &dyn Fn(ToolsProgress)) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    list.iter().try_for_each(|d| install_one(dir, d, on_progress))
}

fn install_one(dir: &Path, d: &Download, on_progress: &dyn Fn(ToolsProgress)) -> Result<(), String> {
    let tmp = dir.join(format!("{}.download", d.label));
    download_to(d, &tmp, on_progress)?;
    let result = match d.payload {
        Payload::File(name) => replace_file(&tmp, &dir.join(name)),
        Payload::Zip(entries) => extract(&tmp, dir, entries),
    };
    let _ = std::fs::remove_file(&tmp);
    if result.is_ok() && d.label == "ffmpeg" {
        std::fs::write(dir.join(FFMPEG_BUILD_MARKER), FFMPEG_BUILD).map_err(|e| e.to_string())?;
    }
    result
}

fn download_to(d: &Download, dest: &Path, on_progress: &dyn Fn(ToolsProgress)) -> Result<(), String> {
    let resp = ureq::get(d.url)
        .call()
        .map_err(|e| format!("下載 {} 失敗：{e}", d.label))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut downloaded = 0u64;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("下載 {} 中斷：{e}", d.label))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        downloaded += n as u64;
        if last_emit.elapsed() > Duration::from_millis(200) {
            last_emit = Instant::now();
            on_progress(ToolsProgress { label: d.label.into(), downloaded, total });
        }
    }
    on_progress(ToolsProgress { label: d.label.into(), downloaded, total: downloaded });
    if total > 0 && downloaded != total {
        return Err(format!("下載 {} 不完整（{downloaded}/{total}）", d.label));
    }
    Ok(())
}

fn extract(zip_path: &Path, dir: &Path, entries: &[(&str, &str)]) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("壓縮檔損壞：{e}"))?;
    for (suffix, dest_name) in entries {
        let index = (0..archive.len())
            .find(|&i| {
                archive
                    .by_index(i)
                    .map(|f| f.name().replace('\\', "/").ends_with(suffix))
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("壓縮檔裡找不到 {suffix}"))?;
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!("{dest_name}.extract"));
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        drop(out);
        replace_file(&tmp, &dir.join(dest_name))?;
    }
    Ok(())
}

fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(from, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    if to.exists() {
        std::fs::remove_file(to).map_err(|e| format!("無法覆蓋 {}（是否正在使用中？）：{e}", to.display()))?;
    }
    std::fs::rename(from, to).map_err(|e| e.to_string())
}

// --- 影片資訊與縮圖 ---

#[tauri::command]
pub async fn probe_video(app: AppHandle, path: String) -> Result<ProbeInfo, String> {
    let ffprobe = require_tool(&app, FFPROBE)?;
    tauri::async_runtime::spawn_blocking(move || {
        let out = command(&ffprobe)
            .args(video::probe_args(&path))
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("讀不到影片資訊：{}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        video::parse_probe_json(&String::from_utf8_lossy(&out.stdout))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 回傳 data URL（`data:image/jpeg;base64,...`），失敗的那張回傳空字串。
#[tauri::command]
/// `start` / `duration` 為要取樣的範圍（有裁切時只取裁切後那段）。
pub async fn video_thumbnails(
    app: AppHandle,
    path: String,
    start: f64,
    duration: f64,
    count: usize,
) -> Result<Vec<String>, String> {
    let ffmpeg = require_tool(&app, FFMPEG)?;
    let times: Vec<f64> = video::thumbnail_times(duration, count.min(60))
        .into_iter()
        .map(|t| t + start)
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        let mut results = vec![String::new(); times.len()];
        // 一次跑 4 個，長影片也只要幾秒
        for (chunk_index, chunk) in times.chunks(4).enumerate() {
            let handles: Vec<_> = chunk
                .iter()
                .map(|&t| {
                    let (ffmpeg, path) = (ffmpeg.clone(), path.clone());
                    std::thread::spawn(move || {
                        command(&ffmpeg)
                            .args(video::thumbnail_args(&path, t, 90))
                            .stderr(Stdio::null())
                            .output()
                            .ok()
                            .filter(|o| o.status.success() && !o.stdout.is_empty())
                            .map(|o| {
                                format!(
                                    "data:image/jpeg;base64,{}",
                                    base64::engine::general_purpose::STANDARD.encode(o.stdout)
                                )
                            })
                            .unwrap_or_default()
                    })
                })
                .collect();
            for (i, h) in handles.into_iter().enumerate() {
                results[chunk_index * 4 + i] = h.join().unwrap_or_default();
            }
        }
        Ok(results)
    })
    .await
    .map_err(|e| e.to_string())?
}

// --- 硬體編碼偵測 ---

#[derive(Clone, Serialize)]
pub struct EncoderAttempt {
    encoder: Encoder,
    label: &'static str,
    ok: bool,
    /// 失敗時 ffmpeg 的錯誤訊息（例如「驅動版本太舊」），設定頁顯示用
    error: String,
}

#[derive(Clone, Serialize)]
pub struct EncoderReport {
    chosen: Encoder,
    attempts: Vec<EncoderAttempt>,
}

/// 依序試 NVIDIA → Intel → AMD → Windows Media Foundation，都不行就用 CPU。
/// 每一個的成敗和錯誤訊息都回傳，偵測失敗時才看得出原因。結果記住，`force` 才重測。
#[tauri::command]
pub async fn detect_encoder(app: AppHandle, state: State<'_, VideoState>, force: Option<bool>) -> Result<EncoderReport, String> {
    if !force.unwrap_or(false) {
        if let Some(r) = state.encoder.lock().unwrap().clone() {
            return Ok(r);
        }
    }
    let ffmpeg = require_tool(&app, FFMPEG)?;
    let report = tauri::async_runtime::spawn_blocking(move || {
        let mut attempts = Vec::new();
        let mut chosen = None;
        for enc in Encoder::HARDWARE {
            let (ok, error) = match command(&ffmpeg).args(enc.probe_args()).stdout(Stdio::null()).output() {
                Ok(o) if o.status.success() => (true, String::new()),
                Ok(o) => (false, explain_encoder_error(&key_error_lines(&String::from_utf8_lossy(&o.stderr)))),
                Err(e) => (false, e.to_string()),
            };
            attempts.push(EncoderAttempt { encoder: enc, label: enc.label(), ok, error });
            if ok {
                chosen = Some(enc);
                break;
            }
        }
        EncoderReport { chosen: chosen.unwrap_or(Encoder::X264), attempts }
    })
    .await
    .map_err(|e| e.to_string())?;
    *state.encoder.lock().unwrap() = Some(report.clone());
    Ok(report)
}

/// 把 ffmpeg 常見的硬體編碼錯誤翻成白話，原始訊息放後面方便回報。
fn explain_encoder_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let hint = if lower.contains("driver does not support") || lower.contains("minimum required nvidia driver") {
        "NVIDIA 驅動版本太舊，請更新顯示卡驅動"
    } else if lower.contains("no capable devices") || lower.contains("cuda_error_no_device") || lower.contains("cannot load nvcuda") || lower.contains("cannot load nvencodeapi") {
        "這台電腦沒有可用的 NVIDIA 顯示卡"
    } else if lower.contains("amfrt64") || lower.contains("amf") && lower.contains("failed to") {
        "這台電腦沒有可用的 AMD 顯示卡"
    } else if lower.contains("mfx") || lower.contains("qsv") || lower.contains("libvpl") {
        "Intel 內顯不支援，或沒有安裝 Intel 顯示驅動"
    } else if lower.contains("encoder not found") || lower.contains("unrecognized option") {
        "這個系統或 ffmpeg 不支援此編碼器"
    } else if lower.contains("no hardware mft") || lower.contains("could not find") && lower.contains("mft") {
        "Windows 找不到可用的硬體編碼器"
    } else {
        ""
    };
    if hint.is_empty() { raw.to_string() } else { format!("{hint}（{raw}）") }
}

/// ffmpeg 失敗時最後幾行通常是「Task finished / Terminating / Nothing was written」這類收尾訊息，
/// 真正的原因（例如「minimum required Nvidia driver is 610」）在前面。去掉收尾訊息，留前兩句。
fn key_error_lines(text: &str) -> String {
    const NOISE: [&str; 6] = [
        "task finished with error code",
        "terminating thread with return code",
        "nothing was written into output file",
        "conversion failed",
        "could not open encoder before eof",
        "error sending frames to consumers",
    ];
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let lower = l.to_lowercase();
            !NOISE.iter().any(|n| lower.contains(n))
        })
        .collect();
    if lines.is_empty() {
        return last_lines(text, 2);
    }
    // 去掉「[h264_nvenc @ 0000…] 」這種前綴，訊息比較好讀
    lines
        .iter()
        .take(2)
        .map(|l| match (l.starts_with('['), l.find("] ")) {
            (true, Some(i)) => &l[i + 2..],
            _ => l,
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" / ")
}

// --- YouTube 下載 ---

#[derive(Clone, Serialize)]
struct YtProgress {
    percent: f64,
}

#[tauri::command]
pub async fn download_youtube(app: AppHandle, url: String) -> Result<String, String> {
    let url = url.trim().to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("請貼上完整的 YouTube 網址".to_string());
    }
    let ytdlp = require_tool(&app, YTDLP)?;
    let ffmpeg = require_tool(&app, FFMPEG)?;
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("youtube");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        let template = dir.join("%(id)s.%(ext)s").display().to_string();
        let ffmpeg_dir = ffmpeg.parent().unwrap_or(Path::new(".")).display().to_string();
        let mut child = command(&ytdlp)
            .args(video::ytdlp_args(&url, &template, &ffmpeg_dir))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("無法執行 yt-dlp：{e}"))?;
        let stderr_tail = collect_tail(child.stderr.take().unwrap());

        let (mut file_path, mut id) = (None, None);
        for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
            match video::parse_ytdlp_line(&line) {
                Some(YtdlpLine::Progress(percent)) => {
                    let _ = app.emit("yt-progress", YtProgress { percent });
                }
                Some(YtdlpLine::Id(i)) => id = Some(i),
                Some(YtdlpLine::FilePath(p)) => file_path = Some(p),
                None => {}
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("YouTube 下載失敗：{}", stderr_tail.join().unwrap_or_default()));
        }
        file_path
            .map(PathBuf::from)
            .or_else(|| id.map(|i| dir.join(format!("{i}.mp4"))))
            .filter(|p| p.is_file())
            .map(|p| p.display().to_string())
            .ok_or_else(|| "下載完成但找不到影片檔".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// --- 串接輸出 ---

#[derive(Clone, Serialize)]
struct RenderProgress {
    seconds: f64,
    total: f64,
}

#[derive(Serialize)]
pub struct RenderResult {
    output: String,
    /// 走了快速模式（畫面直接複製、沒有重新編碼）
    fast: bool,
}

#[tauri::command]
pub async fn render_video(app: AppHandle, state: State<'_, VideoState>, spec: RenderSpec) -> Result<RenderResult, String> {
    let ffmpeg = require_tool(&app, FFMPEG)?;
    let fast = video::can_copy_video(&spec);
    let args = if fast {
        let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let list = dir.join("concat.txt");
        std::fs::write(&list, video::concat_list(&spec)).map_err(|e| e.to_string())?;
        video::build_copy_args(&spec, &list.display().to_string())
    } else {
        video::build_render_args(&spec)?
    };
    let total = video::output_duration(&spec);
    let output = spec.output.clone();
    if let Some(dir) = Path::new(&output).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("無法建立輸出資料夾：{e}"))?;
    }

    let slot = state.render.clone();
    let cancelled = state.cancelled.clone();
    cancelled.store(false, Ordering::SeqCst);
    if slot.lock().unwrap().is_some() {
        return Err("已經有一支影片在輸出中".to_string());
    }

    tauri::async_runtime::spawn_blocking(move || {
        let mut child = command(&ffmpeg)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("無法執行 ffmpeg：{e}"))?;
        let stdout = child.stdout.take().unwrap();
        let stderr_tail = collect_tail(child.stderr.take().unwrap());
        *slot.lock().unwrap() = Some(child);

        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(seconds) = video::parse_progress_line(&line) {
                let _ = app.emit("render-progress", RenderProgress { seconds, total });
            }
        }

        let status = slot.lock().unwrap().take().map(|mut c| c.wait());
        if cancelled.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&output);
            return Err("已取消".to_string());
        }
        match status {
            Some(Ok(s)) if s.success() => Ok(RenderResult { output, fast }),
            Some(Ok(_)) => {
                let _ = std::fs::remove_file(&output);
                Err(format!("輸出失敗：{}", stderr_tail.join().unwrap_or_default()))
            }
            Some(Err(e)) => Err(e.to_string()),
            None => Err("輸出程序遺失".to_string()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn cancel_render(state: State<'_, VideoState>) {
    state.cancelled.store(true, Ordering::SeqCst);
    if let Some(child) = state.render.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
}

/// 在背景收集 stderr 最後幾行，失敗時拿來當錯誤訊息。
fn collect_tail(stderr: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut tail = VecDeque::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tail.len() == 12 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        Vec::from(tail).join("\n")
    })
}

/// 每段在成品上的位置與總長（timeline 刻度、章節時間用）。
#[tauri::command]
pub fn plan_video(spec: RenderSpec) -> (Vec<video::Placement>, f64) {
    video::plan(&spec)
}

/// 輸出資料夾 + 檔名（去掉 Windows 不允許的字元）+ `.mp4`
#[tauri::command]
pub fn output_path(dir: String, name: String) -> Result<String, String> {
    let name = video::sanitize_file_name(&name);
    if name.is_empty() {
        return Err("請輸入檔名".to_string());
    }
    let name = name.strip_suffix(".mp4").unwrap_or(&name).to_string();
    Ok(Path::new(&dir).join(format!("{name}.mp4")).display().to_string())
}

#[tauri::command]
pub fn path_exists(path: String) -> bool {
    Path::new(&path).exists()
}

#[tauri::command]
pub fn parse_timecode(input: String) -> Result<Option<f64>, String> {
    keyword_title_core::timecode::parse(&input).map(|t| t.map(|t| t.total_seconds as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 實際從網路下載這個平台的工具並執行一次。很慢（約 150MB），預設不跑：
    ///   KTT_TOOLS_DIR=/some/dir cargo test -p keyword-title-tool -- --ignored --nocapture
    #[test]
    #[ignore]
    fn downloads_and_runs_tools_for_this_platform() {
        let dir = PathBuf::from(std::env::var("KTT_TOOLS_DIR").expect("set KTT_TOOLS_DIR"));
        let list = downloads_for(true, true);
        assert!(!list.is_empty(), "no downloads defined for this platform");
        install_into(&dir, &list, &|p| {
            if p.downloaded == p.total {
                println!("{} done: {} bytes", p.label, p.total);
            }
        })
        .unwrap();
        for (tool, arg) in [(FFMPEG, "-version"), (FFPROBE, "-version"), (YTDLP, "--version")] {
            let out = command(&dir.join(exe_name(tool))).arg(arg).output().unwrap();
            assert!(out.status.success(), "{tool} failed to run");
            println!("{tool}: {}", String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or(""));
        }
    }
}

#[cfg(test)]
mod explain_tests {
    use super::explain_encoder_error;

    #[test]
    fn picks_the_real_cause_not_the_shutdown_noise() {
        let stderr = "[h264_nvenc @ 000000d7bd3e3f80] The minimum required Nvidia driver for nvenc is 610.00 or newer
[vost#0:0/h264_nvenc @ 000000d7bce25d80] [enc:h264_nvenc @ 000000d7bd3d4b80] Error while opening encoder - maybe incorrect parameters such as bit_rate, rate, width or height.
[fc#0 @ 000000d7bc5dcc80] Error sending frames to consumers: Function not implemented
[vost#0:0/h264_nvenc @ 00000255d98af900] Task finished with error code: -22 (Invalid argument)
[vost#0:0/h264_nvenc @ 00000255d98af900] Terminating thread with return code -22 (Invalid argument)
[out#0/null @ 00000255d77a9680] Nothing was written into output file, because at least one of its streams received no packets.";
        let key = super::key_error_lines(stderr);
        assert!(key.starts_with("The minimum required Nvidia driver for nvenc is 610.00"), "{key}");
        assert!(!key.contains("Task finished"));
        assert!(explain_encoder_error(&key).starts_with("NVIDIA 驅動版本太舊"));
    }

    #[test]
    fn common_errors_get_plain_hints() {
        assert!(explain_encoder_error("Driver does not support the required nvenc API version. Required: 13.0 Found: 12.1")
            .starts_with("NVIDIA 驅動版本太舊"));
        assert!(explain_encoder_error("Cannot load nvcuda.dll").starts_with("這台電腦沒有可用的 NVIDIA"));
        assert!(explain_encoder_error("Error initializing an internal MFX session: unsupported (-3)").starts_with("Intel 內顯"));
        assert!(explain_encoder_error("DLL amfrt64.dll failed to open").starts_with("這台電腦沒有可用的 AMD"));
        assert_eq!(explain_encoder_error("something else"), "something else");
    }
}
