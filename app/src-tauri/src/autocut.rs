//! 自動切段：監看側錄資料夾，每錄完一個 10 分鐘檔就掃描；偵測到完整一段就遮蓋出片，
//! 6 段到齊合成全集。判斷規則全在 core（`keyword_title_core::autocut`），這裡負責
//! 執行 ffmpeg／OCR、逐檔快取、段落狀態與出片佇列。
//!
//! 兩條執行緒：監看（輪詢資料夾 → 掃描 → OCR → 規劃）和出片（一次一支 ffmpeg）。
//! 掃描跟出片可以同時跑（實測不衝突，只是互相變慢）。

use crate::video::{command, require_tool};
use keyword_title_core::video::{self, Clip, Encoder, Join, RenderSpec};
use keyword_title_core::autocut::image;
use keyword_title_core::autocut::plan::{self, MaskSeg, Segment};
use keyword_title_core::autocut::stills::StillSet;
use keyword_title_core::autocut::titles::{parse_titles, Titles};
use keyword_title_core::autocut::{cache, frame, secs, tc, FrameInfo, Layout, BAND, BOX, CANVAS_BYTES, FPS, H, LOGO_H, LOGO_W, START_DELAY, W};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};

/// 輪詢間隔（側錄可能在網路磁碟，檔案監聽事件不可靠）
const POLL: Duration = Duration::from_secs(15);
/// 當晚最後一個檔：大小連續這麼久不變才算錄完
const STABLE: Duration = Duration::from_secs(60);
/// 同時跑幾個 OCR
const OCR_THREADS: usize = 6;

// ───────────────────────── 對外型別 ─────────────────────────

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum TitlesSource {
    /// 共用資料夾當天的 `{日期}關鍵標題1-6.txt`（日期 20261022）
    Sync(String),
    File(String),
    /// 畫面上目前的文稿（之後用 `autocut_set_titles` 更新）
    Text(String),
}

fn yes() -> bool {
    true
}

#[derive(Clone, Deserialize)]
pub struct AutocutConfig {
    /// 側錄資料夾
    pub watch_dir: String,
    /// 靜圖資料夾；空白＝跟側錄同資料夾
    #[serde(default)]
    pub stills_dir: String,
    /// 成品資料夾
    pub output_dir: String,
    pub titles: TitlesSource,
    /// 完整就自動出片
    #[serde(default = "yes")]
    pub auto_render: bool,
    /// 6 段齊了合成全集
    #[serde(default = "yes")]
    pub make_all: bool,
    /// 只輸出這幾段（不合成全集）
    #[serde(default)]
    pub only: Option<Vec<usize>>,
    /// 開發用：人工切點（gt_segments.txt），報告會列出差幾格
    #[serde(default)]
    pub gt: Option<String>,
    /// 整批模式（隱藏命令列參數 `--autocut-batch` 用）：資料夾裡的檔都當成已錄完，
    /// 全部掃完、出完片就結束，不等新檔
    #[serde(default)]
    pub batch: bool,
    /// AI 補判（Gemini）：金鑰（畫面上的設定）或金鑰檔（整批模式用）；都沒有＝不用 AI
    #[serde(default)]
    pub ai_key: Option<String>,
    #[serde(default)]
    pub ai_key_file: Option<String>,
    #[serde(default)]
    pub ai_model: Option<String>,

    // --- 成品加工（跟影片頁單集一樣：正片 +6dB、壓水印；YT、promo 不動） ---
    /// 正片音量增益（dB）
    #[serde(default)]
    pub gain_db: f64,
    /// 水印（台標、月份標）：1920×1080 透明 PNG，依序整張蓋上；空的＝不壓
    #[serde(default)]
    pub watermarks: Vec<String>,
    #[serde(default)]
    pub promo_20: String,
    #[serde(default)]
    pub promo_45: String,
    /// 閃白長度（秒）
    #[serde(default = "half")]
    pub transition: f64,
    /// 串接、全集用的編碼器（畫面先偵測好）；失敗退回 CPU
    #[serde(default)]
    pub encoder: Option<Encoder>,
    /// 串接成品檔名的日期（`1020` → `上傳\1020-3.mp4`）
    #[serde(default)]
    pub date_tag: String,
    /// 每段後面接什麼（開始時的設定；之後用 `autocut_set_chain` 改）
    #[serde(default)]
    pub chains: HashMap<usize, Chain>,
}

fn half() -> f64 {
    0.5
}

/// 一段正片後面要接的東西：正片 →（閃白）→ YT →（直接接）→ promo
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chain {
    /// 0＝不接 promo；20／45＝接那支
    #[serde(default)]
    pub promo: u32,
    /// 這段要串 YT（還沒填好 YT 就先等著）
    #[serde(default)]
    pub yt_mode: bool,
    #[serde(default)]
    pub yt: Option<YtClip>,
    /// 正片跟 YT 之間閃白
    #[serde(default = "yes")]
    pub flash: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct YtClip {
    /// 已下載的檔案
    pub path: String,
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub end: Option<f64>,
}

impl Chain {
    fn enabled(&self) -> bool {
        self.promo != 0 || self.yt_mode
    }
}

#[derive(Clone, Serialize, Default)]
pub struct FileStatus {
    name: String,
    /// recording（錄製中）／waiting（等待掃描）／scanning／ocr／done／error
    state: &'static str,
    frames: usize,
}

#[derive(Clone, Serialize, Default)]
pub struct SegStatus {
    n: usize,
    /// waiting／recording／complete／queued／rendering／done／incomplete／failed
    state: &'static str,
    rec_start: Option<f64>,
    rec_end: Option<f64>,
    /// 已錄長度（秒）
    recorded: f64,
    masks: usize,
    output: Option<String>,
    progress: Option<f64>,
    /// 出片後靜圖或判斷結果變了，可以重新出片
    stale: bool,
    error: Option<String>,
    /// 串接（沒設定要接東西＝None）
    chain: Option<ChainStatus>,
}

#[derive(Clone, Serialize, Default)]
pub struct ChainStatus {
    /// need_yt（還沒填 YT）／waiting（等正片）／queued／rendering／done／stale／failed
    state: &'static str,
    progress: Option<f64>,
    output: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct StageInfo {
    name: String,
    /// 開始時間（Unix 毫秒）
    started: u64,
    secs: f64,
}

#[derive(Clone, Serialize, Default)]
pub struct AutocutStatus {
    running: bool,
    files: Vec<FileStatus>,
    segments: Vec<SegStatus>,
    stages: Vec<StageInfo>,
    /// 全集.mp4（合成好才有）
    all: Option<String>,
    /// 全集合成進度 0–1（合成中才有）
    all_progress: Option<f64>,
    message: String,
}

#[derive(Clone, Serialize)]
struct Done {
    /// 第幾段；0＝全集
    n: usize,
    path: String,
    /// 串接成品（不是正片）
    chain: bool,
}

/// 出片佇列的工作
enum Job {
    /// 遮蓋出正片
    Segment(RenderJob),
    /// 正片後面接 YT／promo
    Chain(usize),
}

// ───────────────────────── 共用狀態 ─────────────────────────

/// 出片需要的全部資料（規劃時算好，出片執行緒不碰判斷）
#[derive(Clone)]
struct RenderJob {
    n: usize,
    pieces: Vec<(usize, usize)>,
    masks: Vec<(MaskSeg, String, Layout)>,
    files: Vec<(PathBuf, f64, f64)>,
    out: PathBuf,
    work: PathBuf,
    /// 正片增益（dB）
    gain_db: f64,
    /// 水印 PNG（整張蓋上）
    watermarks: Vec<String>,
    /// 段落切點＋遮罩＋靜圖（＋增益、水印）的指紋：跟出片時不同＝可重新出片
    fingerprint: String,
}

impl RenderJob {
    fn duration(&self) -> f64 {
        self.pieces.iter().map(|(s, e)| secs(e - s)).sum()
    }
}

#[derive(Clone)]
struct PlanSeg {
    n: usize,
    state: &'static str,
    rec_start: Option<f64>,
    rec_end: Option<f64>,
    recorded: f64,
    masks: usize,
    job: Option<RenderJob>,
}

#[derive(Clone)]
enum Render {
    Queued,
    Rendering(f64),
    Done { output: String, fingerprint: String },
    Failed(String),
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    running: AtomicBool,
    files: Mutex<Vec<FileStatus>>,
    plan: Mutex<Vec<PlanSeg>>,
    renders: Mutex<HashMap<usize, Render>>,
    stages: Mutex<Vec<StageInfo>>,
    all: Mutex<Option<String>>,
    message: Mutex<String>,
    report: Mutex<String>,
    /// TitlesSource::Text 的最新內容
    titles_text: Mutex<String>,
    /// 正在跑的掃描／出片 ffmpeg（停止時砍掉）
    scan_child: Mutex<Option<Child>>,
    render_child: Mutex<Option<Child>>,
    jobs: Mutex<Option<Sender<Job>>>,
    config: Mutex<Option<AutocutConfig>>,
    /// 每段後面接什麼（畫面隨時可改）
    chains: Mutex<HashMap<usize, Chain>>,
    /// 串接成品的狀態（Done 的指紋＝正片指紋＋串接設定）
    chain_renders: Mutex<HashMap<usize, Render>>,
    all_progress: Mutex<Option<f64>>,
    /// 整批模式：紀錄也寫進這個檔（沒有畫面可看）
    log_file: Mutex<Option<PathBuf>>,
}

#[derive(Default)]
pub struct AutocutState {
    shared: Mutex<Option<Arc<Shared>>>,
    last: Mutex<Option<Arc<Shared>>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Shared {
    fn status(&self) -> AutocutStatus {
        let renders = self.renders.lock().unwrap().clone();
        let segments = self
            .plan
            .lock()
            .unwrap()
            .iter()
            .map(|p| {
                let mut s = SegStatus {
                    n: p.n,
                    state: p.state,
                    rec_start: p.rec_start,
                    rec_end: p.rec_end,
                    recorded: p.recorded,
                    masks: p.masks,
                    ..Default::default()
                };
                match renders.get(&p.n) {
                    Some(Render::Queued) => s.state = "queued",
                    Some(Render::Rendering(pct)) => {
                        s.state = "rendering";
                        s.progress = Some(*pct);
                    }
                    Some(Render::Done { output, fingerprint }) => {
                        s.state = "done";
                        s.output = Some(output.clone());
                        s.stale = p.job.as_ref().is_some_and(|j| &j.fingerprint != fingerprint);
                    }
                    Some(Render::Failed(e)) => {
                        s.state = "failed";
                        s.error = Some(e.clone());
                    }
                    None => {}
                }
                s.chain = self.chain_status(p.n);
                s
            })
            .collect();
        AutocutStatus {
            running: self.running.load(Ordering::SeqCst),
            files: self.files.lock().unwrap().clone(),
            segments,
            stages: self.stages.lock().unwrap().clone(),
            all: self.all.lock().unwrap().clone(),
            all_progress: *self.all_progress.lock().unwrap(),
            message: self.message.lock().unwrap().clone(),
        }
    }

    fn publish(&self, app: &AppHandle) {
        let _ = app.emit("autocut-status", self.status());
    }

    fn log(&self, app: &AppHandle, line: impl Into<String>) {
        let line = line.into();
        if let Some(p) = self.log_file.lock().unwrap().as_ref() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let t = now_ms() / 1000 % 86_400;
                let _ = writeln!(f, "[{:02}:{:02}:{:02} UTC] {line}", t / 3600, t / 60 % 60, t % 60);
            }
        }
        let _ = app.emit("autocut-log", line);
    }

    /// report.txt＝目前的判斷報告＋各階段耗時
    fn write_report_file(&self, output_dir: &str) {
        let mut r = self.report.lock().unwrap().clone();
        let stages = self.stages.lock().unwrap();
        r.push_str("\n== 各階段耗時 ==\n");
        let all: f64 = stages.iter().map(|s| s.secs).sum();
        for s in stages.iter() {
            r.push_str(&format!("  {}：{:.1} 秒\n", s.name, s.secs));
        }
        r.push_str(&format!("  合計：{all:.1} 秒（約 {:.1} 分鐘）\n", all / 60.0));
        let _ = std::fs::write(Path::new(output_dir).join("report.txt"), r);
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// 這段可以串接了（有設定、需要的 YT 已填、正片出好）→ (設定, 正片成品, 串接指紋)
    fn chain_ready(&self, n: usize) -> Option<(Chain, String, String)> {
        let chain = self.chains.lock().unwrap().get(&n).cloned().filter(Chain::enabled)?;
        if chain.yt_mode && chain.yt.is_none() {
            return None;
        }
        let (output, seg_fp) = match self.renders.lock().unwrap().get(&n) {
            Some(Render::Done { output, fingerprint }) => (output.clone(), fingerprint.clone()),
            _ => return None,
        };
        let cfg = self.config.lock().unwrap().clone()?;
        let promo = match chain.promo {
            20 => cfg.promo_20.as_str(),
            45 => cfg.promo_45.as_str(),
            _ => "",
        };
        let fp = format!("{seg_fp}\n---\n{}\n{promo}\n{}", serde_json::to_string(&chain).unwrap_or_default(), cfg.transition);
        Some((chain, output, fp))
    }

    fn chain_status(&self, n: usize) -> Option<ChainStatus> {
        let chain = self.chains.lock().unwrap().get(&n).cloned().filter(Chain::enabled)?;
        let mut s = ChainStatus::default();
        match self.chain_renders.lock().unwrap().get(&n).cloned() {
            Some(Render::Queued) => s.state = "queued",
            Some(Render::Rendering(p)) => {
                s.state = "rendering";
                s.progress = Some(p);
            }
            Some(Render::Done { output, fingerprint }) => {
                s.output = Some(output);
                let current = self.chain_ready(n).map(|r| r.2);
                s.state = if current.is_some_and(|c| c != fingerprint) { "stale" } else { "done" };
            }
            Some(Render::Failed(e)) => {
                s.state = "failed";
                s.error = Some(e);
            }
            None => s.state = if chain.yt_mode && chain.yt.is_none() { "need_yt" } else { "waiting" },
        }
        Some(s)
    }

    /// 該串接卻還沒串（或設定改過了）就排進出片佇列。`retry`＝失敗過的也重來（畫面按的）
    fn queue_chain(&self, n: usize, retry: bool) {
        let Some((_, _, fp)) = self.chain_ready(n) else { return };
        let Some(cfg) = self.config.lock().unwrap().clone() else { return };
        let out = chain_out(&cfg, n);
        let mut cr = self.chain_renders.lock().unwrap();
        match cr.get(&n) {
            Some(Render::Queued | Render::Rendering(_)) => return,
            Some(Render::Done { fingerprint, .. }) if *fingerprint == fp => return,
            Some(Render::Failed(_)) if !retry => return,
            _ => {}
        }
        // 工具重開：之前串過、設定也一樣
        if out.is_file() && std::fs::read_to_string(chain_marker(&cfg, n)).ok().as_deref() == Some(fp.as_str()) {
            cr.insert(n, Render::Done { output: out.display().to_string(), fingerprint: fp });
            return;
        }
        if let Some(tx) = self.jobs.lock().unwrap().as_ref() {
            cr.insert(n, Render::Queued);
            let _ = tx.send(Job::Chain(n));
        }
    }

    /// 記一個階段的耗時（畫面顯示、報告最後列出）
    fn stage<T>(&self, app: &AppHandle, name: &str, f: impl FnOnce() -> T) -> T {
        let (started, t) = (now_ms(), Instant::now());
        self.log(app, format!("開始：{name}"));
        let r = f();
        let secs = t.elapsed().as_secs_f64();
        self.log(app, format!("結束：{name}（{secs:.1} 秒）"));
        self.stages.lock().unwrap().push(StageInfo { name: name.to_string(), started, secs });
        self.publish(app);
        r
    }
}

// ───────────────────────── 指令 ─────────────────────────

#[tauri::command]
pub fn autocut_start(app: AppHandle, state: State<'_, AutocutState>, config: AutocutConfig) -> Result<(), String> {
    start(app, &state, config)
}

/// 開始監看（畫面按「開始監看」或整批模式）。整批模式跑完會寫好報告並結束程式。
pub fn start(app: AppHandle, state: &AutocutState, config: AutocutConfig) -> Result<(), String> {
    let mut slot = state.shared.lock().unwrap();
    if slot.as_ref().is_some_and(|s| s.running.load(Ordering::SeqCst)) {
        return Err("已經在監看中".into());
    }
    if !Path::new(&config.watch_dir).is_dir() {
        return Err(format!("找不到側錄資料夾：{}", config.watch_dir));
    }
    if config.output_dir.trim().is_empty() {
        return Err("請選擇成品資料夾".into());
    }
    std::fs::create_dir_all(Path::new(&config.output_dir).join(".autocut")).map_err(|e| format!("無法建立成品資料夾：{e}"))?;
    let ffmpeg = require_tool(&app, "ffmpeg")?;
    let ffprobe = require_tool(&app, "ffprobe")?;
    let shared = Arc::new(Shared::default());
    shared.running.store(true, Ordering::SeqCst);
    if let TitlesSource::Text(t) = &config.titles {
        *shared.titles_text.lock().unwrap() = t.clone();
    }
    *shared.config.lock().unwrap() = Some(config.clone());
    *shared.chains.lock().unwrap() = config.chains.clone();
    if config.batch {
        let p = Path::new(&config.output_dir).join(".autocut").join("batch.log");
        let _ = std::fs::remove_file(&p);
        *shared.log_file.lock().unwrap() = Some(p);
    }
    let (tx, rx) = mpsc::channel();
    *shared.jobs.lock().unwrap() = Some(tx);
    *slot = Some(shared.clone());
    *state.last.lock().unwrap() = Some(shared.clone());

    let (app2, shared2, ffmpeg2) = (app.clone(), shared.clone(), ffmpeg.clone());
    let cfg2 = config.clone();
    let render = std::thread::spawn(move || render_loop(&app2, &shared2, &ffmpeg2, &ffprobe, &cfg2, rx));
    let (batch, output_dir) = (config.batch, config.output_dir.clone());
    std::thread::spawn(move || {
        let r = Watcher::new(app.clone(), shared.clone(), config, ffmpeg).and_then(|mut w| w.run());
        if let Err(e) = r {
            *shared.message.lock().unwrap() = e.clone();
            shared.log(&app, format!("★ {e}"));
        }
        // 監看停了：出片佇列送完就收（停止鍵會直接砍掉正在出的片）
        shared.jobs.lock().unwrap().take();
        let _ = render.join();
        for slot in [&shared.renders, &shared.chain_renders] {
            for r in slot.lock().unwrap().values_mut() {
                if matches!(r, Render::Queued | Render::Rendering(_)) {
                    *r = Render::Failed("已停止".into());
                }
            }
        }
        *shared.all_progress.lock().unwrap() = None;
        shared.running.store(false, Ordering::SeqCst);
        shared.publish(&app);
        if batch {
            // 出片、合成全集的耗時也要進報告
            shared.write_report_file(&output_dir);
            shared.log(&app, "整批完成，結束程式");
            app.exit(0);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn autocut_stop(state: State<'_, AutocutState>) {
    if let Some(s) = state.shared.lock().unwrap().as_ref() {
        s.stop.store(true, Ordering::SeqCst);
        for slot in [&s.scan_child, &s.render_child] {
            if let Some(c) = slot.lock().unwrap().as_mut() {
                let _ = c.kill();
            }
        }
    }
}

/// 畫面重新開啟時拿目前狀態（沒在跑時是上一次的結果）
#[tauri::command]
pub fn autocut_status(state: State<'_, AutocutState>) -> AutocutStatus {
    state.last.lock().unwrap().as_ref().map(|s| s.status()).unwrap_or_default()
}

/// 手動重出某段（靜圖更新後）
#[tauri::command]
pub fn autocut_render(app: AppHandle, state: State<'_, AutocutState>, n: usize) -> Result<(), String> {
    let shared = state.shared.lock().unwrap().clone().filter(|s| s.running.load(Ordering::SeqCst)).ok_or("監看已停止，請先按「開始監看」")?;
    let job = shared.plan.lock().unwrap().iter().find(|p| p.n == n).and_then(|p| p.job.clone()).ok_or(format!("第 {n} 段還不完整，不能出片"))?;
    if matches!(shared.renders.lock().unwrap().get(&n), Some(Render::Queued | Render::Rendering(_))) {
        return Err(format!("第 {n} 段已經在出片佇列裡"));
    }
    shared.renders.lock().unwrap().insert(n, Render::Queued);
    shared.jobs.lock().unwrap().as_ref().ok_or("出片已停止")?.send(Job::Segment(job)).map_err(|e| e.to_string())?;
    shared.publish(&app);
    Ok(())
}

/// 改某段後面接的東西（None＝不接）。正片已經出好就馬上排進佇列串接；沒在監看時只回報成功，
/// 畫面會在開始監看時一起送
#[tauri::command]
pub fn autocut_set_chain(app: AppHandle, state: State<'_, AutocutState>, n: usize, chain: Option<Chain>, retry: Option<bool>) {
    let Some(shared) = state.shared.lock().unwrap().clone().filter(|s| s.running.load(Ordering::SeqCst)) else { return };
    match chain {
        Some(c) => shared.chains.lock().unwrap().insert(n, c),
        None => shared.chains.lock().unwrap().remove(&n),
    };
    shared.queue_chain(n, retry.unwrap_or(false));
    shared.publish(&app);
}

/// 等同 report.txt
#[tauri::command]
pub fn autocut_report(state: State<'_, AutocutState>) -> String {
    state.last.lock().unwrap().as_ref().map(|s| s.report.lock().unwrap().clone()).unwrap_or_default()
}

/// 更新畫面上的文稿（TitlesSource::Text 時，下一輪輪詢會用新的）
#[tauri::command]
pub fn autocut_set_titles(state: State<'_, AutocutState>, text: String) {
    if let Some(s) = state.shared.lock().unwrap().as_ref() {
        *s.titles_text.lock().unwrap() = text;
    }
}

/// 用檔案總管打開資料夾（並選取檔案）
#[tauri::command]
pub fn autocut_reveal(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    #[cfg(windows)]
    let r = if p.is_file() { command(Path::new("explorer")).arg(format!("/select,{}", p.display())).spawn() } else { command(Path::new("explorer")).arg(p).spawn() };
    #[cfg(target_os = "macos")]
    let r = if p.is_file() { command(Path::new("open")).arg("-R").arg(p).spawn() } else { command(Path::new("open")).arg(p).spawn() };
    #[cfg(not(any(windows, target_os = "macos")))]
    let r = command(Path::new("xdg-open")).arg(if p.is_file() { p.parent().unwrap_or(p) } else { p }).spawn();
    r.map(|_| ()).map_err(|e| format!("無法開啟：{e}"))
}

// ───────────────────────── 監看 ─────────────────────────

/// 一個側錄檔的掃描結果
struct Scanned {
    name: String,
    path: PathBuf,
    size: u64,
    frames: Vec<FrameInfo>,
    /// 這個檔裡第幾格 → 標題條文字
    ocr: BTreeMap<usize, String>,
}

struct Watcher {
    app: AppHandle,
    shared: Arc<Shared>,
    cfg: AutocutConfig,
    ffmpeg: PathBuf,
    work: PathBuf,
    logo: image::GrayImage,
    stills: StillSet,
    stills_key: String,
    signature: String,
    titles: Titles,
    scanned: Vec<Scanned>,
    /// 最後一個檔：(大小, 從什麼時候開始沒變)
    sizes: HashMap<String, (u64, Instant)>,
    /// 段落開頭（未修準）→ Logo 轉場結束的格；None＝找不到
    refined: HashMap<usize, Option<usize>>,
    /// 掃描失敗的檔 → 當時的大小
    failed: HashMap<String, u64>,
    /// 規劃用的資料有沒有變（新掃好的檔、靜圖、文稿）
    dirty: bool,
    last_titles: String,
    /// AI 補判（沒設金鑰＝None）與判斷過的結果（存在 .autocut\ai.tsv，同一個地方不重複問）
    ai: Option<ai::Gemini>,
    ai_cache: HashMap<String, (bool, String)>,
}

impl Watcher {
    fn new(app: AppHandle, shared: Arc<Shared>, cfg: AutocutConfig, ffmpeg: PathBuf) -> Result<Watcher, String> {
        let logo = image::load_from_memory(include_bytes!("../assets/bumper_logo.png")).map_err(|e| format!("讀不到 Logo 樣板：{e}"))?;
        let work = Path::new(&cfg.output_dir).join(".autocut");
        let key = match (&cfg.ai_key, &cfg.ai_key_file) {
            (Some(k), _) if !k.trim().is_empty() => Some(k.trim().to_string()),
            (_, Some(f)) => Some(std::fs::read_to_string(f).map_err(|e| format!("讀不到 AI 金鑰檔：{e}"))?.trim().to_string()).filter(|k| !k.is_empty()),
            _ => None,
        };
        let model = cfg.ai_model.clone().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "gemini-3.5-flash-lite".into());
        let ai_cache = ai::load_cache(&work.join("ai.tsv"));
        Ok(Watcher {
            ai: key.map(|k| ai::Gemini::new(k, model)),
            ai_cache,
            app,
            shared,
            cfg,
            ffmpeg,
            work,
            logo: frame::logo_template(&logo),
            stills: StillSet::default(),
            stills_key: String::new(),
            signature: String::new(),
            titles: Titles::new(),
            scanned: Vec::new(),
            sizes: HashMap::new(),
            refined: HashMap::new(),
            failed: HashMap::new(),
            dirty: true,
            last_titles: String::new(),
        })
    }

    fn log(&self, line: impl Into<String>) {
        self.shared.log(&self.app, line);
    }

    fn run(&mut self) -> Result<(), String> {
        self.log(format!("開始監看 {}", self.cfg.watch_dir));
        match &self.ai {
            Some(g) => self.log(format!("AI 補判：開啟（{}，已判斷過 {} 處）", g.model, self.ai_cache.len())),
            None => self.log("AI 補判：關閉（沒有設定金鑰）"),
        }
        while !self.shared.stopped() {
            // 停止時砍掉 ffmpeg 造成的錯誤不用報
            match self.poll().or_else(|e| if self.shared.stopped() { Ok(false) } else { Err(e) }) {
                // 整批模式：資料夾裡的檔都處理過就結束，出片佇列由呼叫端等它送完
                Ok(all_done) if self.cfg.batch && all_done => break,
                Ok(_) => {}
                Err(e) => {
                    *self.shared.message.lock().unwrap() = e.clone();
                    self.log(format!("★ {e}"));
                    self.shared.publish(&self.app);
                    if self.cfg.batch {
                        break;
                    }
                }
            }
            // 每秒看一次要不要停
            let t = Instant::now();
            while t.elapsed() < POLL && !self.shared.stopped() {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        // 停止：還在錄製中（缺頭或尾）的段落標不完整
        for p in self.shared.plan.lock().unwrap().iter_mut() {
            if p.state == "recording" {
                p.state = "incomplete";
            }
        }
        self.write_report();
        self.log("已停止監看");
        Ok(())
    }

    /// 掃描資料夾一輪。回傳：資料夾裡的檔是不是都處理過了
    fn poll(&mut self) -> Result<bool, String> {
        self.load_stills()?;
        self.load_titles()?;
        let files = self.list_files()?;
        // 依序處理錄完的檔；中間任何一個還沒錄完，後面的都等（時間軸要連續）
        let mut statuses: Vec<FileStatus> = Vec::new();
        let mut ready = true;
        for (k, (name, path, size)) in files.iter().enumerate() {
            if self.shared.stopped() {
                return Ok(false);
            }
            let finished = k + 1 < files.len() || self.cfg.batch || self.stable(name, *size);
            if !ready || !finished {
                ready = false;
                statuses.push(FileStatus { name: name.clone(), state: if finished { "waiting" } else { "recording" }, frames: 0 });
                continue;
            }
            // 掃描或 OCR 失敗過的檔不要每一輪重試（大小變了才再試）；時間軸斷在這裡
            if self.failed.get(name) == Some(size) {
                ready = false;
                statuses.push(FileStatus { name: name.clone(), state: "error", frames: 0 });
                continue;
            }
            let have = self.scanned.get(k).is_some_and(|s| &s.name == name && s.size == *size);
            if !have {
                self.scanned.truncate(k);
                *self.shared.files.lock().unwrap() = statuses.iter().cloned().chain(std::iter::once(FileStatus { name: name.clone(), state: "scanning", frames: 0 })).collect();
                self.shared.publish(&self.app);
                match self.scan(name, path, *size) {
                    Ok(s) => self.scanned.push(s),
                    Err(e) => {
                        if !self.shared.stopped() {
                            self.failed.insert(name.clone(), *size);
                        }
                        *self.shared.files.lock().unwrap() = statuses.iter().cloned().chain(std::iter::once(FileStatus { name: name.clone(), state: "error", frames: 0 })).collect();
                        return Err(format!("{name}：{e}"));
                    }
                }
                self.dirty = true;
            }
            statuses.push(FileStatus { name: name.clone(), state: "done", frames: self.scanned[k].frames.len() });
            *self.shared.files.lock().unwrap() = statuses.clone();
            // 掃好一個就規劃一次，段落一完整就能出片，不用等後面的檔
            if self.dirty {
                self.replan();
            }
        }
        // 資料夾裡的檔變少（被搬走）→ 丟掉多的
        let done = statuses.iter().filter(|s| s.state == "done").count();
        if self.scanned.len() > done {
            self.scanned.truncate(done);
            self.dirty = true;
        }
        // 都處理過（失敗的也算，整批模式不會卡住）
        let all_done = statuses.iter().all(|s| s.state == "done" || s.state == "error");
        *self.shared.files.lock().unwrap() = statuses;
        if self.dirty {
            self.replan();
        }
        self.shared.publish(&self.app);
        Ok(all_done)
    }

    /// 最後一個檔：大小連續 60 秒不變才算錄完
    fn stable(&mut self, name: &str, size: u64) -> bool {
        let e = self.sizes.entry(name.to_string()).or_insert((size, Instant::now()));
        if e.0 != size {
            *e = (size, Instant::now());
        }
        size > 0 && e.1.elapsed() >= STABLE
    }

    /// 側錄檔清單（依檔名裡的時間排序）：(檔名, 路徑, 大小)
    fn list_files(&self) -> Result<Vec<(String, PathBuf, u64)>, String> {
        let rd = std::fs::read_dir(&self.cfg.watch_dir).map_err(|e| format!("讀不到側錄資料夾：{e}"))?;
        let mut all: Vec<(String, PathBuf, u64)> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let ok = Path::new(&name).extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4"));
                ok.then(|| (name, e.path(), e.metadata().map_or(0, |m| m.len())))
            })
            .collect();
        // 有側錄檔名時間的只取那些（成品跟側錄放同一個資料夾也不會被當成側錄）；都沒有就全部依檔名
        if all.iter().any(|f| cache::name_timestamp(&f.0).is_some()) {
            all.retain(|f| cache::name_timestamp(&f.0).is_some());
            all.sort_by_key(|f| (cache::name_timestamp(&f.0), f.0.clone()));
        } else {
            all.retain(|f| !is_output_name(&f.0));
            all.sort_by(|a, b| a.0.cmp(&b.0));
        }
        Ok(all)
    }

    fn load_stills(&mut self) -> Result<(), String> {
        let dir = if self.cfg.stills_dir.trim().is_empty() { self.cfg.watch_dir.clone() } else { self.cfg.stills_dir.clone() };
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("讀不到靜圖資料夾：{e}"))?;
        let mut files: Vec<(String, PathBuf, String)> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                let ext = p.extension().and_then(|x| x.to_str()).map(|x| x.to_lowercase());
                if !matches!(ext.as_deref(), Some("jpg" | "jpeg" | "png")) {
                    return None;
                }
                let m = e.metadata().ok()?;
                let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
                Some((e.file_name().to_string_lossy().to_string(), p, format!("{}:{mtime}", m.len())))
            })
            .collect();
        files.sort_by(|a, b| a.1.cmp(&b.1));
        let key: String = files.iter().map(|f| format!("{}|{}\n", f.0, f.2)).collect();
        if key == self.stills_key {
            return Ok(());
        }
        let first = self.stills_key.is_empty() && self.signature.is_empty();
        let set = self.shared.stage(&self.app, "讀取靜圖", || {
            let images: Vec<(String, String, image::RgbImage)> = files
                .iter()
                .filter_map(|(name, p, _)| match image::open(p) {
                    Ok(img) => Some((name.clone(), p.display().to_string(), img.to_rgb8())),
                    Err(e) => {
                        self.log(format!("★ 讀不到靜圖 {name}：{e}"));
                        None
                    }
                })
                .collect();
            StillSet::build(images)
        });
        self.stills_key = key;
        let sig = set.signature();
        self.log(format!("靜圖 {} 張、來源字樣板 {} 個", set.stills.len(), set.templates.len()));
        self.stills = set;
        if sig != self.signature {
            if !first {
                self.log("靜圖改過，重新掃描已掃過的檔（OCR 沿用）");
            }
            self.signature = sig;
            // 靜圖改了：每格跟每張靜圖的相似度都要重算，已掃過的檔全部重掃
            self.scanned.clear();
            self.refined.clear();
            self.dirty = true;
        }
        Ok(())
    }

    fn load_titles(&mut self) -> Result<(), String> {
        let text = match &self.cfg.titles {
            TitlesSource::Text(_) => self.shared.titles_text.lock().unwrap().clone(),
            TitlesSource::File(p) => read_titles_file(Path::new(p))?,
            TitlesSource::Sync(date) => {
                let settings = crate::get_settings(self.app.clone());
                let p = keyword_title_core::sync::file_path(&settings, date).ok_or("共用資料夾設定不完整，讀不到文稿")?;
                read_titles_file(&p)?
            }
        };
        if text != self.last_titles {
            self.titles = parse_titles(&text);
            self.last_titles = text;
            self.dirty = true;
            if self.titles.is_empty() {
                self.log("★ 文稿裡找不到任何一段標題");
            }
        }
        Ok(())
    }

    // ───── 掃描一個檔 ─────

    fn cache_paths(&self, name: &str) -> (PathBuf, PathBuf) {
        (self.work.join(format!("{name}.cache")), self.work.join(format!("{name}.ocr.tsv")))
    }

    fn scan(&mut self, name: &str, path: &Path, size: u64) -> Result<Scanned, String> {
        let (cache_path, ocr_path) = self.cache_paths(name);
        let ocr_cached = std::fs::read_to_string(&ocr_path).ok().map(|s| cache::decode_ocr(&s));
        // 快取能用就不掃
        if let (Ok(b), Some(ocr)) = (std::fs::read(&cache_path), ocr_cached.as_ref()) {
            match cache::decode(&b, size, &self.signature) {
                Ok(frames) => {
                    self.log(format!("{name}：沿用掃描快取（{} 格）", frames.len()));
                    return Ok(Scanned { name: name.into(), path: path.into(), size, frames, ocr: ocr.clone() });
                }
                Err(why) => self.log(format!("{name}：快取不能沿用（{why}），重新掃描")),
            }
        }
        let ocr_dir = self.work.join(format!("{name}.ocr"));
        let want_ocr = ocr_cached.is_none();
        if want_ocr {
            let _ = std::fs::remove_dir_all(&ocr_dir);
            std::fs::create_dir_all(&ocr_dir).map_err(|e| e.to_string())?;
        }
        let frames = self.shared.stage(&self.app, &format!("掃描 {name}"), || self.analyze(path, want_ocr.then_some(ocr_dir.as_path())))?;
        if self.shared.stopped() {
            return Err("已停止".into());
        }
        let ocr = match ocr_cached {
            Some(o) => o,
            None => {
                let ocr = self.shared.stage(&self.app, &format!("OCR {name}"), || run_ocr(&ocr_dir))?;
                std::fs::write(&ocr_path, cache::encode_ocr(&ocr)).map_err(|e| format!("寫不進 OCR 結果：{e}"))?;
                let _ = std::fs::remove_dir_all(&ocr_dir);
                ocr
            }
        };
        std::fs::write(&cache_path, cache::encode(size, &self.signature, self.stills.stills.len(), &frames)).map_err(|e| format!("寫不進掃描快取：{e}"))?;
        Ok(Scanned { name: name.into(), path: path.into(), size, frames, ocr })
    }

    /// 解碼一個側錄檔、逐格判斷。`ocr_dir` 有給就同時每 2 秒截一張標題條（bar_000000.png 起）
    fn analyze(&self, path: &Path, ocr_dir: Option<&Path>) -> Result<Vec<FrameInfo>, String> {
        let mut cmd = command(&self.ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-threads", "0", "-i"])
            .arg(path)
            .args(["-filter_complex", &keyword_title_core::autocut::scan_filter(ocr_dir.is_some())])
            .args(["-map", "[out]", "-an", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
        if let Some(dir) = ocr_dir {
            cmd.args(["-map", "[ocr]", "-start_number", "0"]).arg(dir.join("bar_%06d.png"));
        }
        let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("無法執行 ffmpeg：{e}"))?;
        let mut out = child.stdout.take().unwrap();
        let err_tail = collect_stderr(child.stderr.take().unwrap());
        *self.shared.scan_child.lock().unwrap() = Some(child);
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let batch = threads * 8;
        let mut frames: Vec<FrameInfo> = Vec::new();
        // 檔與檔交界的第一格沒有前一格可比，cut_diff 記 0（實測交界不在鏡頭切換上）
        let mut prev: Option<image::GrayImage> = None;
        let stills = &self.stills;
        loop {
            let mut bufs: Vec<Vec<u8>> = Vec::with_capacity(batch);
            while bufs.len() < batch {
                let mut b = vec![0u8; CANVAS_BYTES];
                if out.read_exact(&mut b).is_err() {
                    break;
                }
                bufs.push(b);
            }
            if bufs.is_empty() {
                break;
            }
            let chunk = bufs.len().div_ceil(threads);
            let results: Vec<(FrameInfo, image::GrayImage)> = std::thread::scope(|sc| {
                let hs: Vec<_> = bufs.chunks(chunk).map(|part| sc.spawn(|| part.iter().map(|b| frame::judge(b, stills)).collect::<Vec<_>>())).collect();
                hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
            });
            let eof = results.len() < batch;
            for (mut info, whole) in results {
                info.cut_diff = prev.as_ref().map(|p| frame::frame_diff(p, &whole)).unwrap_or(0.0);
                prev = Some(whole);
                frames.push(info);
            }
            if eof || self.shared.stopped() {
                break;
            }
        }
        let status = self.shared.scan_child.lock().unwrap().take().map(|mut c| c.wait());
        if self.shared.stopped() {
            return Err("已停止".into());
        }
        let ok = matches!(status, Some(Ok(s)) if s.success());
        if !ok && frames.is_empty() {
            return Err(format!("掃描失敗：{}", err_tail.join().unwrap_or_default()));
        }
        Ok(frames)
    }

    // ───── 規劃 ─────

    /// 所有已掃好的檔接成一條時間軸，規劃段落與遮罩，更新狀態、把完整的段落送去出片
    fn replan(&mut self) {
        self.dirty = false;
        let t = Instant::now();
        let started = now_ms();
        let mut frames: Vec<FrameInfo> = Vec::new();
        let mut ocr: BTreeMap<usize, String> = BTreeMap::new();
        let mut files: Vec<(PathBuf, f64, f64)> = Vec::new();
        for s in &self.scanned {
            let off = frames.len();
            // 每 2 秒一張：第 k 張＝這個檔第 2k 秒
            ocr.extend(s.ocr.iter().map(|(k, text)| (off + ((*k * 2) as f64 * FPS) as usize, text.clone())));
            // 整體時間軸＝各檔格數依序相加（不用秒數相加，每檔是 17,984 格＝600.066 秒）
            files.push((s.path.clone(), secs(off), secs(s.frames.len())));
            frames.extend(s.frames.iter().cloned());
        }
        let mut log = String::new();
        let mut segs = plan::plan_segments(&frames, &ocr, &self.titles, &mut log);
        // 完整的段落：開頭對到 Logo 轉場結束那一格（要解碼一小段，結果記起來）
        for seg in segs.iter_mut().filter(|s| s.complete()) {
            let s = seg.start();
            let k = match self.refined.get(&s) {
                Some(k) => *k,
                None => {
                    let k = self.refine_start(&files, s);
                    self.refined.insert(s, k);
                    k
                }
            };
            match k {
                Some(s2) => {
                    let s2 = s2 + START_DELAY;
                    log.push_str(&format!("  第 {} 段開頭：{} → {}（Logo 轉場結束）\n", seg.n, tc(secs(s)), tc(secs(s2))));
                    seg.pieces[0].0 = s2;
                }
                None => log.push_str(&format!("  第 {} 段開頭：找不到 Logo 轉場，維持 {}\n", seg.n, tc(secs(s)))),
            }
        }
        let mut planned: Vec<(Segment, Vec<MaskSeg>)> = segs
            .into_iter()
            .map(|s| {
                let m = if s.complete() { plan::segment_masks(&frames, &self.stills, &s) } else { Vec::new() };
                (s, m)
            })
            .collect();
        // AI 補判：規則沒把握的地方截右上角來源字給 Gemini 看（判斷過的存起來，不重複問）
        if self.ai.is_some() {
            let mut asked = 0usize;
            let t_ai = (now_ms(), Instant::now());
            for (seg, masks) in planned.iter_mut().filter(|(s, _)| s.complete()) {
                let cands = plan::ai_candidates(&frames, &self.stills, seg, masks);
                let answers = self.ai_decide_many(&files, &cands);
                for (c, a) in cands.iter().zip(answers) {
                    let (same, note, fresh) = match a {
                        Ok(x) => x,
                        Err(e) => {
                            log.push_str(&format!("  ★ AI 補判失敗（第 {} 段 側錄 {}）：{e}\n", seg.n, tc(secs(c.start))));
                            continue;
                        }
                    };
                    asked += fresh as usize;
                    let kind = if c.kind == plan::AiKind::Gap { "空檔" } else { "灰色地帶" };
                    log.push_str(&format!(
                        "  AI：第 {} 段 側錄 {} – {}（{kind}）→ {}｜{note}\n",
                        seg.n,
                        tc(secs(c.start)),
                        tc(secs(c.end)),
                        if same { "同一個來源，遮" } else { "不同，不遮" }
                    ));
                    if same {
                        plan::apply_ai(&frames, masks, c);
                    }
                }
            }
            if asked > 0 {
                self.shared.stages.lock().unwrap().push(StageInfo { name: format!("AI 補判（問了 {asked} 處）"), started: t_ai.0, secs: t_ai.1.elapsed().as_secs_f64() });
            }
        }

        // 段落狀態：文稿有的每一段都列出來
        let wanted = |n: usize| self.cfg.only.as_ref().is_none_or(|o| o.contains(&n));
        let mut ns: Vec<usize> = self.titles.keys().copied().collect();
        ns.extend(planned.iter().map(|(s, _)| s.n).filter(|n| !self.titles.contains_key(n)));
        ns.sort();
        let total = frames.len();
        let mut plan_segs = Vec::new();
        for n in ns {
            let Some((seg, masks)) = planned.iter().find(|(s, _)| s.n == n) else {
                plan_segs.push(PlanSeg { n, state: "waiting", rec_start: None, rec_end: None, recorded: 0.0, masks: 0, job: None });
                continue;
            };
            let state = if !seg.start_ok {
                "incomplete"
            } else if seg.complete() {
                "complete"
            } else {
                "recording"
            };
            let job = (seg.complete() && wanted(n)).then(|| self.job(seg, masks, &files));
            plan_segs.push(PlanSeg {
                n,
                state,
                rec_start: Some(secs(seg.start())),
                rec_end: seg.end_ok.then(|| secs(seg.end())),
                recorded: if seg.end_ok { seg.duration() } else { secs(total.saturating_sub(seg.start())) },
                masks: masks.len(),
                job,
            });
        }
        // 送去出片：完整、還沒出過的段落。之前出過（工具重開）且判斷一樣就直接算完成
        let mut to_send = Vec::new();
        {
            let mut renders = self.shared.renders.lock().unwrap();
            for p in &plan_segs {
                let Some(job) = &p.job else { continue };
                if renders.contains_key(&p.n) {
                    continue;
                }
                if let Some(fp) = done_marker(job) {
                    renders.insert(p.n, Render::Done { output: job.out.display().to_string(), fingerprint: fp });
                } else if self.cfg.auto_render {
                    renders.insert(p.n, Render::Queued);
                    to_send.push(job.clone());
                }
            }
        }
        let ns: Vec<usize> = plan_segs.iter().map(|p| p.n).collect();
        *self.shared.plan.lock().unwrap() = plan_segs;
        self.shared.stages.lock().unwrap().push(StageInfo { name: "規劃段落與遮罩".into(), started, secs: t.elapsed().as_secs_f64() });
        if let Some(tx) = self.shared.jobs.lock().unwrap().as_ref() {
            for job in to_send {
                self.log(format!("第 {} 段完整，送去出片", job.n));
                let _ = tx.send(Job::Segment(job));
            }
        }
        // 工具重開時之前出好的正片：該串接的補串（串過且設定一樣的直接算完成）
        for n in ns {
            self.shared.queue_chain(n, false);
        }

        // 報告
        let mut r = self.stills.report();
        r.push_str(&format!("\n== 文稿：{} 段 ==\n", self.titles.len()));
        for (n, l) in &self.titles {
            r.push_str(&format!("  第 {n} 段：{} 行標題\n", l.len()));
        }
        r.push_str(&log);
        r.push_str(&plan::report_segments(&planned, &self.stills));
        if !self.stills.stills.is_empty() && !frames.is_empty() {
            r.push_str(&plan::report_coverage(&frames, &planned, &self.stills));
        }
        if let Some(gt) = self.cfg.gt.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            r.push_str(&plan::report_gt(&gt, &planned.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>()));
        }
        r.push_str(&format!("\n側錄 {} 個檔、{}\n", self.scanned.len(), tc(secs(total))));
        *self.shared.report.lock().unwrap() = r;
        self.write_report();
        self.shared.publish(&self.app);
    }

    fn write_report(&self) {
        self.shared.write_report_file(&self.cfg.output_dir);
    }

    fn job(&self, seg: &Segment, masks: &[MaskSeg], files: &[(PathBuf, f64, f64)]) -> RenderJob {
        let masks: Vec<(MaskSeg, String, Layout)> = masks
            .iter()
            .map(|m| {
                let s = &self.stills.stills[m.still];
                (m.clone(), s.path.clone(), s.layout)
            })
            .collect();
        let mut fingerprint = format!(
            "{:?}\n{}",
            seg.pieces,
            masks.iter().map(|(m, p, _)| format!("{} {} {:?} {}", m.start, m.end, m.layout, p)).collect::<Vec<_>>().join("\n")
        );
        let watermarks: Vec<String> = self.cfg.watermarks.iter().filter(|p| !p.trim().is_empty()).cloned().collect();
        // 沒加工時指紋跟舊版一樣（之前出好的不用重出）
        if self.cfg.gain_db != 0.0 {
            fingerprint.push_str(&format!("\ngain {}", self.cfg.gain_db));
        }
        for p in &watermarks {
            let mtime = std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            fingerprint.push_str(&format!("\nwatermark {p} {mtime}"));
        }
        // 只取這段用得到的檔（出片時不用開整天的檔）
        let (a, b) = (secs(seg.start()), secs(seg.end()));
        let files = files.iter().filter(|(_, fs, fd)| fs + fd > a && *fs < b).cloned().collect();
        RenderJob {
            n: seg.n,
            pieces: seg.pieces.clone(),
            masks,
            files,
            out: Path::new(&self.cfg.output_dir).join(format!("第{}段.mp4", seg.n)),
            work: self.work.clone(),
            gain_db: self.cfg.gain_db,
            watermarks,
            fingerprint,
        }
    }

    /// 問 AI 這些地方的來源字跟參考靜圖是不是同一個（判斷過的直接用；沒判斷過的 4 個同時問）。
    /// 每個回傳 (同一個?, 說明, 這次有沒有真的呼叫 API)
    fn ai_decide_many(&mut self, files: &[(PathBuf, f64, f64)], cands: &[plan::AiCandidate]) -> Vec<Result<(bool, String, bool), String>> {
        let key_of = |c: &plan::AiCandidate| {
            let names: Vec<&str> = c.stills.iter().map(|&k| self.stills.stills[k].name.as_str()).collect();
            format!("{}|{}|{:?}|{:?}|{}", c.start, c.end, c.kind, c.layout, names.join(","))
        };
        let keys: Vec<String> = cands.iter().map(key_of).collect();
        let mut out: Vec<Option<Result<(bool, String, bool), String>>> = keys.iter().map(|k| self.ai_cache.get(k).map(|(s, n)| Ok((*s, n.clone(), false)))).collect();
        let todo: Vec<usize> = (0..cands.len()).filter(|&i| out[i].is_none()).collect();
        if !todo.is_empty() {
            let Some(gemini) = self.ai.as_ref() else { return vec![Err("AI 沒開".into()); cands.len()] };
            let (ffmpeg, stills) = (&self.ffmpeg, &self.stills);
            let chunk = todo.len().div_ceil(AI_THREADS);
            let fresh: Vec<(usize, Result<(bool, String), String>)> = std::thread::scope(|sc| {
                let hs: Vec<_> = todo
                    .chunks(chunk)
                    .map(|part| sc.spawn(move || part.iter().map(|&i| (i, ai_eval(ffmpeg, stills, gemini, files, &cands[i]))).collect::<Vec<_>>()))
                    .collect();
                hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            });
            for (i, r) in fresh {
                if let Ok((same, note)) = &r {
                    self.ai_cache.insert(keys[i].clone(), (*same, note.clone()));
                    ai::append_cache(&self.work.join("ai.tsv"), &keys[i], *same, note);
                }
                out[i] = Some(r.map(|(s, n)| (s, n, true)));
            }
        }
        out.into_iter().map(|r| r.unwrap_or_else(|| Err("沒有結果".into()))).collect()
    }

    /// 段落開頭修準：在標題條出現前 6 秒內，找「關鍵時刻」Logo 轉場溶接到棚內結束的那一格。
    /// 只解碼這一小段（80×45 灰階）。
    fn refine_start(&self, files: &[(PathBuf, f64, f64)], s: usize) -> Option<usize> {
        let (s0, count) = plan::refine_window(s);
        let t0 = secs(s0);
        let (p, fs, _) = files.iter().rev().find(|(_, fs, _)| *fs <= t0)?;
        let o = command(&self.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{:.4}", t0 - fs), "-i"])
            .arg(p)
            .args(["-frames:v", &count.to_string(), "-an", "-vf", &format!("scale={LOGO_W}:{LOGO_H}:flags=area,format=gray"), "-f", "rawvideo", "-"])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        let frames: Vec<image::GrayImage> = o.stdout.chunks_exact((LOGO_W * LOGO_H) as usize).filter_map(|c| image::GrayImage::from_raw(LOGO_W, LOGO_H, c.to_vec())).collect();
        plan::logo_end(&self.logo, &frames).map(|k| s0 + k)
    }
}

fn is_output_name(name: &str) -> bool {
    name == "全集.mp4" || name.strip_prefix('第').and_then(|s| s.strip_suffix("段.mp4")).is_some_and(|n| n.parse::<usize>().is_ok())
}

fn read_titles_file(p: &Path) -> Result<String, String> {
    let b = std::fs::read(p).map_err(|e| format!("讀不到文稿 {}：{e}", p.display()))?;
    String::from_utf8(b).map_err(|_| format!("文稿不是 UTF-8 文字檔：{}", p.display()))
}

fn collect_stderr(stderr: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut s);
        s.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
    })
}

// ───────────────────────── AI 補判（Gemini） ─────────────────────────

/// 同時問 AI 幾個地方
const AI_THREADS: usize = 4;

/// 一個地方：截參考靜圖與側錄 3 格的右上角給 AI，過半說「相同」才算
fn ai_eval(ffmpeg: &Path, stills: &StillSet, gemini: &ai::Gemini, files: &[(PathBuf, f64, f64)], c: &plan::AiCandidate) -> Result<(bool, String), String> {
    let refs: Vec<Vec<u8>> = c.stills.iter().take(2).map(|&k| ai::still_crop(&stills.stills[k].path, c.layout)).collect::<Result<_, _>>()?;
    let targets: Vec<Vec<u8>> = c.samples.iter().map(|&f| grab_crop(ffmpeg, files, f, c.layout)).collect::<Result<_, _>>()?;
    let ans = gemini.compare(&refs, &targets)?;
    let same = ans.same.iter().filter(|b| **b).count() * 2 > ans.same.len().max(1);
    let note = format!("參考「{}」、讀到 {}", ans.reference, ans.texts.iter().map(|t| format!("「{t}」")).collect::<Vec<_>>().join(""));
    Ok((same, note))
}

/// 截側錄某一格的右上角來源字（PNG）
fn grab_crop(ffmpeg: &Path, files: &[(PathBuf, f64, f64)], frame: usize, layout: Layout) -> Result<Vec<u8>, String> {
    let t = secs(frame);
    let (p, fs, _) = files.iter().rev().find(|(_, fs, _)| *fs <= t).ok_or("找不到這一格的側錄檔")?;
    let (x, y, w, h, k) = ai::crop_box(layout);
    let o = command(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{:.4}", t - fs), "-i"])
        .arg(p)
        .args(["-frames:v", "1", "-an", "-vf", &format!("crop={w}:{h}:{x}:{y},scale={}:{}:flags=lanczos", w * k, h * k), "-f", "image2pipe", "-vcodec", "png", "-"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("無法執行 ffmpeg：{e}"))?;
    if o.stdout.is_empty() {
        return Err("截不到畫面".into());
    }
    Ok(o.stdout)
}

mod ai {
    use base64::Engine as _;
    use keyword_title_core::autocut::image::{self, imageops, ImageFormat};
    use keyword_title_core::autocut::{Layout, H, W};
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Duration;

    pub struct Gemini {
        key: String,
        pub model: String,
    }

    /// 這組金鑰其他工具也在用，上限每分鐘 15 次：這裡最多每分鐘 10 次（所有執行緒共用），留餘裕給別人
    const MAX_PER_MINUTE: u64 = 10;
    static LAST_CALL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

    fn wait_turn() {
        let gap = Duration::from_millis(60_000 / MAX_PER_MINUTE);
        let mut last = LAST_CALL.lock().unwrap();
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < gap {
                std::thread::sleep(gap - elapsed);
            }
        }
        *last = Some(std::time::Instant::now());
    }

    pub struct Answer {
        pub reference: String,
        pub texts: Vec<String>,
        pub same: Vec<bool>,
    }

    /// 給 AI 看的右上角範圍 x y w h 與放大倍數（比規則用的範圍寬：一行長字、兩行字都放得進去）
    pub fn crop_box(layout: Layout) -> (u32, u32, u32, u32, u32) {
        match layout {
            // 雙框：左框右上角（右界不超過左框 x 942）；字小，放大 2 倍
            Layout::Dual => (560, 70, 382, 160, 2),
            Layout::Full => (1280, 40, 640, 210, 1),
        }
    }

    /// 靜圖的同一塊（PNG）
    pub fn still_crop(path: &str, layout: Layout) -> Result<Vec<u8>, String> {
        let img = image::open(path).map_err(|e| format!("讀不到靜圖 {path}：{e}"))?.to_rgb8();
        let img = if img.dimensions() != (W, H) { imageops::resize(&img, W, H, imageops::FilterType::Triangle) } else { img };
        let (x, y, w, h, k) = crop_box(layout);
        let crop = imageops::crop_imm(&img, x, y, w, h).to_image();
        let crop = imageops::resize(&crop, w * k, h * k, imageops::FilterType::Lanczos3);
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(crop).write_to(&mut out, ImageFormat::Png).map_err(|e| e.to_string())?;
        Ok(out.into_inner())
    }

    impl Gemini {
        pub fn new(key: String, model: String) -> Gemini {
            Gemini { key, model }
        }

        /// 參考圖（同一個來源的靜圖）＋要判斷的幾格 → 每格的來源字是不是跟參考相同
        pub fn compare(&self, refs: &[Vec<u8>], targets: &[Vec<u8>]) -> Result<Answer, String> {
            let b64 = |b: &Vec<u8>| base64::engine::general_purpose::STANDARD.encode(b);
            let prompt = format!(
                "以下是台灣電視節目畫面右上角「來源標示」區域的截圖。來源標示是白色或灰色的小字，例如「翻攝 IG / 伊朗女忍者」「翻攝 Youtube 少康戰情室」「翻攝 抖音 / 中國辦公大樓空置」，也可能只有一行說明文字；截圖裡可能還有別台的台標、字幕，那些不算。\n\
                 第 1～{r} 張是參考圖（同一個來源）。第 {a}～{b} 張是要判斷的畫面。\n\
                 請先讀出參考圖的來源標示文字，再逐張判斷要判斷的畫面裡是否出現「完全相同」的來源標示（平台和主題都要相同；畫面內容、背景不同沒關係）。看不到任何來源標示就算不同。\n\
                 只回傳 JSON：{{\"ref\":\"參考圖的來源標示\",\"targets\":[{{\"text\":\"讀到的來源標示，沒有就空字串\",\"same\":true}}]}}，targets 依序共 {n} 個。",
                r = refs.len(),
                a = refs.len() + 1,
                b = refs.len() + targets.len(),
                n = targets.len()
            );
            let mut parts = vec![serde_json::json!({ "text": prompt })];
            for img in refs.iter().chain(targets) {
                parts.push(serde_json::json!({ "inline_data": { "mime_type": "image/png", "data": b64(img) } }));
            }
            let body = serde_json::json!({
                "contents": [{ "role": "user", "parts": parts }],
                "generationConfig": { "temperature": 0, "response_mime_type": "application/json" }
            });
            let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", self.model);
            let body = body.to_string();
            let mut tries = 0;
            let text = loop {
                wait_turn();
                let resp = ureq::post(&url).set("x-goog-api-key", &self.key).set("Content-Type", "application/json").timeout(Duration::from_secs(90)).send_string(&body);
                match resp {
                    Ok(r) => break r.into_string().map_err(|e| e.to_string())?,
                    // 超過每分鐘上限（別的工具也在用）：等一下再試
                    Err(ureq::Error::Status(429, _)) if tries < 3 => {
                        tries += 1;
                        std::thread::sleep(Duration::from_secs(20));
                    }
                    Err(ureq::Error::Status(code, r)) => {
                        let detail = r.into_string().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&detail).ok().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or(detail);
                        return Err(format!("Gemini 回應 {code}：{}", msg.chars().take(300).collect::<String>()));
                    }
                    Err(e) => return Err(format!("連不到 Gemini：{e}")),
                }
            };
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("Gemini 回應看不懂：{e}"))?;
            let out = v["candidates"][0]["content"]["parts"][0]["text"].as_str().ok_or("Gemini 沒有回答")?;
            let a: serde_json::Value = serde_json::from_str(out.trim().trim_start_matches("```json").trim_end_matches("```").trim()).map_err(|e| format!("Gemini 回答不是 JSON：{e}"))?;
            let targets = a["targets"].as_array().cloned().unwrap_or_default();
            Ok(Answer {
                reference: a["ref"].as_str().unwrap_or("").to_string(),
                texts: targets.iter().map(|t| t["text"].as_str().unwrap_or("").to_string()).collect(),
                same: targets.iter().map(|t| t["same"].as_bool().unwrap_or(false)).collect(),
            })
        }
    }

    /// ai.tsv：每行「位置<Tab>1|0<Tab>說明」
    pub fn load_cache(path: &Path) -> HashMap<String, (bool, String)> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut it = l.splitn(3, '\t');
                let k = it.next()?.to_string();
                let same = it.next()? == "1";
                Some((k, (same, it.next().unwrap_or("").to_string())))
            })
            .collect()
    }

    pub fn append_cache(path: &Path, key: &str, same: bool, note: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{key}\t{}\t{}", if same { 1 } else { 0 }, note.replace(['\t', '\n', '\r'], " "));
        }
    }
}

// ───────────────────────── OCR ─────────────────────────

/// 資料夾裡的 bar_000000.png… 丟給 Windows 內建 OCR（zh-Hant-TW，幾條執行緒同時跑）→ 第幾張 → 文字
fn run_ocr(dir: &Path) -> Result<BTreeMap<usize, String>, String> {
    let mut imgs: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let k = p.file_stem()?.to_str()?.strip_prefix("bar_")?.parse().ok()?;
            Some((k, p))
        })
        .collect();
    imgs.sort();
    if imgs.is_empty() {
        return Ok(BTreeMap::new());
    }
    let chunk = imgs.len().div_ceil(OCR_THREADS);
    let parts: Vec<Result<Vec<(usize, String)>, String>> = std::thread::scope(|sc| {
        let hs: Vec<_> = imgs
            .chunks(chunk)
            .map(|part| {
                sc.spawn(move || {
                    let engine = ocr::Engine::new()?;
                    part.iter()
                        .map(|(k, p)| {
                            let img = image::open(p).map_err(|e| format!("讀不到 {}：{e}", p.display()))?.to_rgb8();
                            Ok((*k, engine.recognize(&img)?))
                        })
                        .collect()
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("OCR 執行緒當掉".into()))).collect()
    });
    let mut out = BTreeMap::new();
    for p in parts {
        out.extend(p?);
    }
    Ok(out)
}

#[cfg(windows)]
mod ocr {
    use keyword_title_core::autocut::image::RgbImage;
    use windows::core::HSTRING;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    pub struct Engine(OcrEngine);

    impl Engine {
        pub fn new() -> Result<Engine, String> {
            let lang = Language::CreateLanguage(&HSTRING::from("zh-Hant-TW")).map_err(|e| e.to_string())?;
            OcrEngine::TryCreateFromLanguage(&lang).map(Engine).map_err(|_| {
                "系統沒有繁體中文 OCR（zh-Hant-TW）。請到 Windows「設定 → 時間與語言 → 語言」新增「中文（台灣）」並安裝「光學字元辨識」。".to_string()
            })
        }

        /// 每行去掉空白、行與行用一個空格接起來
        pub fn recognize(&self, img: &RgbImage) -> Result<String, String> {
            let e = |e: windows::core::Error| format!("OCR 失敗：{e}");
            let (w, h) = img.dimensions();
            let mut bgra = Vec::with_capacity((w * h * 4) as usize);
            for p in img.pixels() {
                bgra.extend([p[2], p[1], p[0], 255]);
            }
            let writer = DataWriter::new().map_err(e)?;
            writer.WriteBytes(&bgra).map_err(e)?;
            let buf = writer.DetachBuffer().map_err(e)?;
            let bmp = SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Bgra8, w as i32, h as i32).map_err(e)?;
            let res = self.0.RecognizeAsync(&bmp).map_err(e)?.get().map_err(e)?;
            let mut lines = Vec::new();
            for line in res.Lines().map_err(e)? {
                lines.push(line.Text().map_err(e)?.to_string().chars().filter(|c| !c.is_whitespace()).collect::<String>());
            }
            Ok(lines.join(" "))
        }
    }
}

#[cfg(not(windows))]
mod ocr {
    use keyword_title_core::autocut::image::RgbImage;

    /// 開發用（Mac）：沒有 Windows OCR。可以先把 Windows 跑出來的 .ocr.tsv 放進 .autocut 沿用。
    pub struct Engine;

    impl Engine {
        pub fn new() -> Result<Engine, String> {
            Err("OCR 只支援 Windows（使用 Windows 內建的繁體中文 OCR）".into())
        }

        pub fn recognize(&self, _img: &RgbImage) -> Result<String, String> {
            unreachable!()
        }
    }
}

// ───────────────────────── 出片 ─────────────────────────

fn marker_path(job: &RenderJob) -> PathBuf {
    job.work.join(format!("第{}段.done", job.n))
}

/// 之前出過這段（成品還在）→ 回傳當時的指紋
fn done_marker(job: &RenderJob) -> Option<String> {
    if !job.out.is_file() {
        return None;
    }
    std::fs::read_to_string(marker_path(job)).ok()
}

/// 串接成品：`成品資料夾\上傳\1020-3.mp4`（沒有日期時 `第3段.mp4`）。放子資料夾，
/// 成品跟側錄同資料夾時也不會被當成側錄檔
fn chain_out(cfg: &AutocutConfig, n: usize) -> PathBuf {
    let name = if cfg.date_tag.trim().is_empty() { format!("第{n}段.mp4") } else { format!("{}-{n}.mp4", video::sanitize_file_name(cfg.date_tag.trim())) };
    Path::new(&cfg.output_dir).join("上傳").join(name)
}

fn chain_marker(cfg: &AutocutConfig, n: usize) -> PathBuf {
    Path::new(&cfg.output_dir).join(".autocut").join(format!("第{n}段.chain.done"))
}

fn render_loop(app: &AppHandle, shared: &Arc<Shared>, ffmpeg: &Path, ffprobe: &Path, cfg: &AutocutConfig, rx: Receiver<Job>) {
    let mut pending: std::collections::VecDeque<Job> = Default::default();
    loop {
        let job = match pending.pop_front() {
            Some(j) => j,
            None => match rx.recv() {
                Ok(j) => j,
                Err(_) => break,
            },
        };
        if shared.stopped() {
            break;
        }
        match job {
            Job::Segment(job) => {
                let n = job.n;
                if render_one_segment(app, shared, ffprobe, ffmpeg, &job) && shared.chain_ready(n).is_some() {
                    // 正片好了直接串（不經佇列：整批模式這時佇列可能已經關了）
                    shared.chain_renders.lock().unwrap().insert(n, Render::Queued);
                    run_chain(app, shared, ffmpeg, ffprobe, cfg, n);
                }
            }
            Job::Chain(n) => run_chain(app, shared, ffmpeg, ffprobe, cfg, n),
        }
        // 全集要重新編碼、很久：佇列裡還有工作（正片、串接）就先做完
        pending.extend(rx.try_iter());
        if pending.is_empty() && cfg.make_all && cfg.only.is_none() && !shared.stopped() {
            maybe_concat_all(app, shared, ffmpeg, ffprobe, cfg);
        }
    }
}

/// 出一段正片；成功回傳 true
fn render_one_segment(app: &AppHandle, shared: &Arc<Shared>, ffprobe: &Path, ffmpeg: &Path, job: &RenderJob) -> bool {
    {
        let n = job.n;
        shared.renders.lock().unwrap().insert(n, Render::Rendering(0.0));
        shared.publish(app);
        let r = shared.stage(app, &format!("輸出第 {n} 段（去交錯、遮蓋、編碼）"), || render_segment(app, shared, ffmpeg, job));
        let r = r.and_then(|_| verify_length(ffprobe, &job.out, job.duration()));
        let ok = r.is_ok();
        match r {
            Ok(()) => {
                let _ = std::fs::write(marker_path(job), &job.fingerprint);
                let output = job.out.display().to_string();
                shared.renders.lock().unwrap().insert(n, Render::Done { output: output.clone(), fingerprint: job.fingerprint.clone() });
                shared.log(app, format!("第 {n} 段完成：{output}"));
                let _ = app.emit("autocut-done", Done { n, path: output, chain: false });
            }
            Err(e) => {
                let e = if shared.stopped() { "已停止".to_string() } else { e };
                shared.log(app, format!("★ 第 {n} 段輸出失敗：{e}"));
                shared.renders.lock().unwrap().insert(n, Render::Failed(e));
            }
        }
        shared.publish(app);
        ok
    }
}

/// 第 n 段正片後面接 YT／promo，輸出上傳用的成品
fn run_chain(app: &AppHandle, shared: &Arc<Shared>, ffmpeg: &Path, ffprobe: &Path, cfg: &AutocutConfig, n: usize) {
    // 排隊時設定可能被改掉（取消串接、YT 清掉）
    let Some((chain, seg_out, fp)) = shared.chain_ready(n) else {
        shared.chain_renders.lock().unwrap().remove(&n);
        shared.publish(app);
        return;
    };
    let out = chain_out(cfg, n);
    if out.is_file() && std::fs::read_to_string(chain_marker(cfg, n)).ok().as_deref() == Some(fp.as_str()) {
        shared.chain_renders.lock().unwrap().insert(n, Render::Done { output: out.display().to_string(), fingerprint: fp });
        shared.publish(app);
        return;
    }
    shared.chain_renders.lock().unwrap().insert(n, Render::Rendering(0.0));
    shared.publish(app);
    let what = [chain.yt.as_ref().map(|_| if chain.flash { "閃白＋YT" } else { "YT" }), (chain.promo != 0).then_some(if chain.promo == 20 { "promo 20 秒" } else { "promo 45 秒" })]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("＋");
    let r = shared.stage(app, &format!("串接第 {n} 段（{what}）"), || render_chain(app, shared, ffmpeg, ffprobe, cfg, n, &chain, &seg_out, &out));
    match r {
        Ok(()) => {
            let _ = std::fs::write(chain_marker(cfg, n), &fp);
            let output = out.display().to_string();
            shared.chain_renders.lock().unwrap().insert(n, Render::Done { output: output.clone(), fingerprint: fp });
            shared.log(app, format!("第 {n} 段串接完成：{output}"));
            let _ = app.emit("autocut-done", Done { n, path: output, chain: true });
        }
        Err(e) => {
            let e = if shared.stopped() { "已停止".to_string() } else { e };
            shared.log(app, format!("★ 第 {n} 段串接失敗：{e}"));
            shared.chain_renders.lock().unwrap().insert(n, Render::Failed(e));
        }
    }
    shared.publish(app);
    // 串接途中設定又改了 → 再排一次
    if !shared.stopped() {
        shared.queue_chain(n, false);
    }
}

fn probe(ffprobe: &Path, path: &str) -> Result<video::ProbeInfo, String> {
    let out = command(ffprobe).args(video::probe_args(path)).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("讀不到 {path}：{}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    video::parse_probe_json(&String::from_utf8_lossy(&out.stdout))
}

fn clip_of(ffprobe: &Path, path: &str, label: &str, join: Join, start: Option<f64>, end: Option<f64>) -> Result<Clip, String> {
    if !Path::new(path).is_file() {
        return Err(format!("找不到「{label}」的檔案：{path}"));
    }
    let info = probe(ffprobe, path)?;
    Ok(Clip { path: path.to_string(), duration: info.duration, has_audio: info.has_audio, start, end, join, signature: Some(info.copy_signature), label: Some(label.to_string()), gain_db: 0.0 })
}

fn promo_path(cfg: &AutocutConfig, secs: u32) -> Result<&str, String> {
    let p = if secs == 20 { &cfg.promo_20 } else { &cfg.promo_45 };
    if p.trim().is_empty() {
        return Err(format!("設定頁還沒指定 {secs} 秒 promo 的檔案"));
    }
    Ok(p)
}

/// 正片 →（閃白）→ YT →（直接接）→ promo。正片已經 +6dB、壓過水印，這裡都不再加
#[allow(clippy::too_many_arguments)]
fn render_chain(app: &AppHandle, shared: &Arc<Shared>, ffmpeg: &Path, ffprobe: &Path, cfg: &AutocutConfig, n: usize, chain: &Chain, seg_out: &str, out: &Path) -> Result<(), String> {
    let mut clips = vec![clip_of(ffprobe, seg_out, &format!("第 {n} 段"), Join::Cut, None, None)?];
    if let Some(yt) = &chain.yt {
        clips.push(clip_of(ffprobe, &yt.path, "YT", if chain.flash { Join::Flash } else { Join::Cut }, yt.start, yt.end)?);
    }
    if chain.promo != 0 {
        clips.push(clip_of(ffprobe, promo_path(cfg, chain.promo)?, &format!("Promo {} 秒", chain.promo), Join::Cut, None, None)?);
    }
    let dir = out.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("無法建立 {}：{e}", dir.display()))?;
    let tmp = out.with_extension("part.mp4");
    let spec = RenderSpec { clips, transition: cfg.transition, width: W as u32, height: H as u32, fps: "30000/1001".into(), encoder: cfg.encoder.unwrap_or(Encoder::X264), output: tmp.display().to_string() };
    let list = Path::new(&cfg.output_dir).join(".autocut").join(format!("第{n}段.chain.txt"));
    let r = render_spec(app, shared, ffmpeg, spec, &list, &mut |p| {
        shared.chain_renders.lock().unwrap().insert(n, Render::Rendering(p));
    });
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, out).map_err(|e| format!("無法寫入 {}：{e}", out.display()))
}

/// 用影片頁同一套參數輸出。能直接複製畫面就走快速模式；硬體編碼失敗退回 CPU
fn render_spec(app: &AppHandle, shared: &Arc<Shared>, ffmpeg: &Path, mut spec: RenderSpec, list: &Path, on_progress: &mut dyn FnMut(f64)) -> Result<(), String> {
    let total = video::output_duration(&spec);
    let mut progress = |p: f64| {
        on_progress(p);
        shared.publish(app);
    };
    if video::can_copy_video(&spec) {
        std::fs::write(list, video::concat_list(&spec)).map_err(|e| e.to_string())?;
        let r = run_ffmpeg(shared, ffmpeg, &video::build_copy_args(&spec, &list.display().to_string()), total, &mut progress);
        let _ = std::fs::remove_file(list);
        return r;
    }
    match run_ffmpeg(shared, ffmpeg, &video::build_render_args(&spec)?, total, &mut progress) {
        Err(e) if spec.encoder != Encoder::X264 && !shared.stopped() => {
            shared.log(app, format!("{} 編碼失敗，改用 CPU：{}", spec.encoder.label(), e.lines().last().unwrap_or("")));
            spec.encoder = Encoder::X264;
            run_ffmpeg(shared, ffmpeg, &video::build_render_args(&spec)?, total, &mut progress)
        }
        r => r,
    }
}

/// 跑一支 ffmpeg（停止鍵砍得到），`-progress pipe:1` 的進度每半秒回報一次
fn run_ffmpeg(shared: &Shared, ffmpeg: &Path, args: &[String], total: f64, on_progress: &mut dyn FnMut(f64)) -> Result<(), String> {
    let mut child = command(ffmpeg).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("無法執行 ffmpeg：{e}"))?;
    let stdout = child.stdout.take().unwrap();
    let err_tail = collect_stderr(child.stderr.take().unwrap());
    *shared.render_child.lock().unwrap() = Some(child);
    let mut last = Instant::now();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(t) = video::parse_progress_line(&line) {
            if last.elapsed() > Duration::from_millis(500) && total > 0.0 {
                last = Instant::now();
                on_progress((t / total).clamp(0.0, 1.0));
            }
        }
    }
    let status = shared.render_child.lock().unwrap().take().map(|mut c| c.wait());
    match status {
        Some(Ok(s)) if s.success() => Ok(()),
        _ => Err(err_tail.join().unwrap_or_default()),
    }
}

/// 文稿的每一段都出好（而且不是舊的）→ 合成全集。
/// 有設定 promo：跟影片頁全集一樣（第一段 → promo 20 → 閃白接各段 → promo 45，重新編碼）；
/// 沒設定 promo（整批測試）：直接接，不重新編碼
fn maybe_concat_all(app: &AppHandle, shared: &Arc<Shared>, ffmpeg: &Path, ffprobe: &Path, cfg: &AutocutConfig) {
    let plan = shared.plan.lock().unwrap().clone();
    let renders = shared.renders.lock().unwrap().clone();
    if plan.is_empty() {
        return;
    }
    let mut clips = Vec::new();
    let mut fps = Vec::new();
    for p in &plan {
        match (renders.get(&p.n), &p.job) {
            (Some(Render::Done { output, fingerprint }), Some(job)) if &job.fingerprint == fingerprint => {
                clips.push(PathBuf::from(output));
                fps.push(fingerprint.clone());
            }
            _ => return,
        }
    }
    let out = Path::new(&cfg.output_dir).join("全集.mp4");
    let marker = Path::new(&cfg.output_dir).join(".autocut").join("全集.done");
    let with_promo = !cfg.promo_20.trim().is_empty() || !cfg.promo_45.trim().is_empty();
    let mut fp = fps.join("\n---\n");
    if with_promo {
        fp.push_str(&format!("\n---\n{}\n{}\n{}", cfg.promo_20, cfg.promo_45, cfg.transition));
    }
    if out.is_file() && std::fs::read_to_string(&marker).ok().as_deref() == Some(fp.as_str()) {
        *shared.all.lock().unwrap() = Some(out.display().to_string());
        return;
    }
    let ok = if with_promo {
        shared.stage(app, "合成全集（promo、閃白，重新編碼）", || render_all(app, shared, ffmpeg, ffprobe, cfg, &clips, &out))
    } else {
        shared.stage(app, "合成全集（直接接，不重新編碼）", || concat_all(ffmpeg, &clips, &out))
    };
    *shared.all_progress.lock().unwrap() = None;
    match ok {
        Ok(()) => {
            let _ = std::fs::write(&marker, fp);
            let path = out.display().to_string();
            *shared.all.lock().unwrap() = Some(path.clone());
            shared.log(app, format!("全集完成：{path}"));
            let _ = app.emit("autocut-done", Done { n: 0, path, chain: false });
        }
        Err(e) => {
            let e = if shared.stopped() { "已停止".to_string() } else { e };
            shared.log(app, format!("★ 合成全集失敗：{e}"));
        }
    }
    shared.publish(app);
}

/// 全集跟影片頁一樣：第一段 → promo 20（直接接）→ 各段之間閃白 → promo 45（直接接）。
/// 正片已經 +6dB、壓過水印；promo 沒設定的就略過
fn render_all(app: &AppHandle, shared: &Arc<Shared>, ffmpeg: &Path, ffprobe: &Path, cfg: &AutocutConfig, segs: &[PathBuf], out: &Path) -> Result<(), String> {
    let mut clips = Vec::new();
    for (i, p) in segs.iter().enumerate() {
        clips.push(clip_of(ffprobe, &p.display().to_string(), &format!("第 {} 段", i + 1), Join::Flash, None, None)?);
        if i == 0 && !cfg.promo_20.trim().is_empty() {
            clips.push(clip_of(ffprobe, &cfg.promo_20, "Promo 20 秒", Join::Cut, None, None)?);
        }
    }
    if !cfg.promo_45.trim().is_empty() {
        clips.push(clip_of(ffprobe, &cfg.promo_45, "Promo 45 秒", Join::Cut, None, None)?);
    }
    let tmp = out.with_extension("part.mp4");
    *shared.all_progress.lock().unwrap() = Some(0.0);
    let spec = RenderSpec { clips, transition: cfg.transition, width: W as u32, height: H as u32, fps: "30000/1001".into(), encoder: cfg.encoder.unwrap_or(Encoder::X264), output: tmp.display().to_string() };
    let list = Path::new(&cfg.output_dir).join(".autocut").join("全集.list.txt");
    let r = render_spec(app, shared, ffmpeg, spec, &list, &mut |p| {
        *shared.all_progress.lock().unwrap() = Some(p);
    });
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, out).map_err(|e| format!("無法寫入 {}：{e}", out.display()))
}

/// 依片段從各側錄檔取出來接起來，去交錯、蓋上靜圖、編碼。先試 NVENC，不行改 x264
fn render_segment(app: &AppHandle, shared: &Shared, ffmpeg: &Path, job: &RenderJob) -> Result<(), String> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into(), "-loglevel".into(), "error".into()];
    let mut n_in = 0usize;
    let mut cat = String::new();
    // 成品時間軸：每個片段接在前一個後面
    let mut offsets: Vec<(usize, usize, f64)> = Vec::new();
    let mut acc = 0.0;
    for &(s, e) in &job.pieces {
        let (t0, t1) = (secs(s), secs(e));
        for (p, fs, fd) in &job.files {
            let (a, b) = ((t0 - fs).max(0.0), (t1 - fs).min(*fd));
            if b - a <= 0.01 {
                continue;
            }
            args.extend(["-ss".into(), format!("{a:.4}"), "-to".into(), format!("{b:.4}"), "-i".into(), p.display().to_string()]);
            cat.push_str(&format!("[{n_in}:v:0][{n_in}:a:0]"));
            n_in += 1;
        }
        offsets.push((s, e, acc));
        acc += t1 - t0;
    }
    if n_in == 0 {
        return Err("找不到這段的側錄檔".into());
    }
    let rel = |f: usize| -> f64 {
        for &(s, e, o) in &offsets {
            if f >= s && f <= e {
                return o + secs(f - s);
            }
        }
        0.0
    };
    let mut g = vec![format!("{cat}concat=n={n_in}:v=1:a=1[vc][ac]"), "[vc]bwdif=mode=send_frame:parity=tff,setsar=1[v0]".to_string()];
    let mut cur = "v0".to_string();
    for (si, (m, still_path, still_layout)) in job.masks.iter().enumerate() {
        args.extend(["-i".into(), still_path.clone()]);
        let inp = n_in + si;
        let en = format!("enable='between(t,{:.4},{:.4})'", rel(m.start), rel(m.end));
        match m.layout {
            Layout::Full => {
                g.push(format!("[{inp}:v]scale={W}:{H},format=yuv420p,split[p{si}a][p{si}b]"));
                g.push(format!("[p{si}a]crop={W}:{}:0:0[t{si}]", BAND.0));
                g.push(format!("[p{si}b]crop={W}:{}:0:{}[b{si}]", H - BAND.1, BAND.1));
                g.push(format!("[{cur}][t{si}]overlay=0:0:{en}[m{si}a]"));
                g.push(format!("[m{si}a][b{si}]overlay=0:{}:{en}[m{si}]", BAND.1));
            }
            Layout::Dual => {
                let (bw, bh) = (BOX.2 - BOX.0, BOX.3.min(BAND.0) - BOX.1);
                let src = if *still_layout == Layout::Dual { format!("scale={W}:{H},crop={bw}:{bh}:{}:{}", BOX.0, BOX.1) } else { format!("scale=-2:{bh},crop={bw}:{bh}") };
                g.push(format!("[{inp}:v]{src},format=yuv420p[d{si}]"));
                g.push(format!("[{cur}][d{si}]overlay={}:{}:{en}[m{si}]", BOX.0, BOX.1));
            }
        }
        cur = format!("m{si}");
    }
    // 水印蓋在最上面（遮罩之後），整張 1920×1080 蓋上
    for (wi, p) in job.watermarks.iter().enumerate() {
        if !Path::new(p).is_file() {
            return Err(format!("找不到水印：{p}"));
        }
        args.extend(["-i".into(), p.clone()]);
        let inp = n_in + job.masks.len() + wi;
        g.push(format!("[{inp}:v]scale={W}:{H},format=rgba[wm{wi}]"));
        g.push(format!("[{cur}][wm{wi}]overlay=0:0[wmo{wi}]"));
        cur = format!("wmo{wi}");
    }
    g.push(format!("[{cur}]null[vout]"));
    // 正片 +6dB：跟影片頁一樣拉高後用限幅器壓住峰值
    let audio = if job.gain_db != 0.0 {
        g.push(format!("[ac]volume={}dB,alimiter=limit=0.95:level=false:latency=1[aout]", job.gain_db));
        "[aout]"
    } else {
        "[ac]"
    };
    let script = job.work.join(format!("第{}段.filter.txt", job.n));
    std::fs::write(&script, g.join(";\n")).map_err(|e| e.to_string())?;
    let total = job.duration();
    let tmp = job.out.with_extension("part.mp4");
    let run = |enc: &[&str]| -> Result<(), String> {
        let mut a = args.clone();
        a.extend(["-filter_complex_script".into(), script.display().to_string(), "-map".into(), "[vout]".into(), "-map".into(), audio.into()]);
        a.extend(enc.iter().map(|s| s.to_string()));
        a.extend(["-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-movflags", "+faststart", "-progress", "pipe:1", "-nostats"].map(String::from));
        a.push(tmp.display().to_string());
        let mut child = command(ffmpeg).args(&a).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("無法執行 ffmpeg：{e}"))?;
        let stdout = child.stdout.take().unwrap();
        let err_tail = collect_stderr(child.stderr.take().unwrap());
        *shared.render_child.lock().unwrap() = Some(child);
        let mut last = Instant::now();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(t) = keyword_title_core::video::parse_progress_line(&line) {
                if last.elapsed() > Duration::from_millis(500) {
                    last = Instant::now();
                    shared.renders.lock().unwrap().insert(job.n, Render::Rendering((t / total).clamp(0.0, 1.0)));
                    shared.publish(app);
                }
            }
        }
        let status = shared.render_child.lock().unwrap().take().map(|mut c| c.wait());
        match status {
            Some(Ok(s)) if s.success() => Ok(()),
            _ => Err(err_tail.join().unwrap_or_default()),
        }
    };
    let r = run(&["-c:v", "h264_nvenc", "-preset", "p5", "-b:v", "12M", "-pix_fmt", "yuv420p", "-r", "30000/1001"]).or_else(|e| {
        if shared.stopped() {
            return Err(e);
        }
        shared.log(app, format!("第 {} 段 NVENC 不能用，改用 CPU 編碼", job.n));
        run(&["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-r", "30000/1001"])
    });
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // 寫好才換上正式檔名，出到一半停掉不會留下半個第N段.mp4
    std::fs::rename(&tmp, &job.out).map_err(|e| format!("無法寫入 {}：{e}", job.out.display()))
}

/// 成品長度跟段落長度差不到 1 秒才算成功
fn verify_length(ffprobe: &Path, out: &Path, expected: f64) -> Result<(), String> {
    let o = command(ffprobe).args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(out).output().map_err(|e| e.to_string())?;
    let d: f64 = String::from_utf8_lossy(&o.stdout).trim().parse().map_err(|_| "讀不到成品長度".to_string())?;
    if (d - expected).abs() > 1.0 {
        return Err(format!("成品長度 {} 跟段落長度 {} 不符", tc(d), tc(expected)));
    }
    Ok(())
}

fn concat_all(ffmpeg: &Path, clips: &[PathBuf], out: &Path) -> Result<(), String> {
    let list = out.with_extension("list.txt");
    let body: String = std::iter::once("ffconcat version 1.0".to_string())
        .chain(clips.iter().map(|p| format!("file '{}'", p.display().to_string().replace('\'', "'\\''"))))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&list, body).map_err(|e| e.to_string())?;
    let o = command(ffmpeg)
        .args(["-hide_banner", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(out)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&list);
    if o.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names_are_not_recordings() {
        assert!(is_output_name("第1段.mp4"));
        assert!(is_output_name("全集.mp4"));
        assert!(!is_output_name("第一段.mp4"));
        assert!(!is_output_name("1022-1NEW.mp4"));
    }

    #[test]
    fn titles_source_json() {
        let c: AutocutConfig = serde_json::from_str(r#"{"watch_dir":"a","output_dir":"b","titles":{"kind":"Sync","value":"20261022"}}"#).unwrap();
        assert!(matches!(c.titles, TitlesSource::Sync(ref d) if d == "20261022"));
        assert!(c.auto_render && c.make_all && c.only.is_none());
    }
}
