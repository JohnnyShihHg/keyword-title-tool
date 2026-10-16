//! 影片串接：組 ffmpeg / yt-dlp 參數、解析它們的輸出。
//!
//! 只做純字串處理，實際執行在 app 端，這裡才能單元測試。

use serde::{Deserialize, Serialize};

/// 串接用的一段素材。`start` / `end` 為選填的裁切範圍（秒）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub path: String,
    /// 原始檔案長度（秒），來自 ffprobe。
    pub duration: f64,
    pub has_audio: bool,
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub end: Option<f64>,
}

impl Clip {
    fn trim_start(&self) -> f64 {
        self.start.unwrap_or(0.0).clamp(0.0, self.duration)
    }

    fn trim_end(&self) -> f64 {
        self.end.unwrap_or(self.duration).clamp(0.0, self.duration)
    }

    /// 裁切後實際使用的長度。
    pub fn used_duration(&self) -> f64 {
        (self.trim_end() - self.trim_start()).max(0.0)
    }

    fn is_trimmed(&self) -> bool {
        self.trim_start() > 0.0 || self.trim_end() < self.duration
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoder {
    /// CPU 軟體編碼，任何電腦都能用
    X264,
    /// NVIDIA 顯示卡
    Nvenc,
    /// Intel 內顯
    Qsv,
    /// AMD 顯示卡
    Amf,
}

impl Encoder {
    /// 自動偵測時依序嘗試的硬體編碼器（都失敗就用 x264）。
    pub const HARDWARE: [Encoder; 3] = [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf];

    fn codec(self) -> &'static str {
        match self {
            Encoder::X264 => "libx264",
            Encoder::Nvenc => "h264_nvenc",
            Encoder::Qsv => "h264_qsv",
            Encoder::Amf => "h264_amf",
        }
    }

    /// 畫質參數：都調在「肉眼看不出差異、檔案不過大」的程度。
    fn quality_args(self) -> &'static [&'static str] {
        match self {
            Encoder::X264 => &["-preset", "veryfast", "-crf", "20"],
            Encoder::Nvenc => &["-preset", "p4", "-rc", "vbr", "-cq", "21", "-b:v", "0"],
            Encoder::Qsv => &["-preset", "medium", "-global_quality", "21"],
            Encoder::Amf => &["-quality", "balanced", "-rc", "cqp", "-qp_i", "20", "-qp_p", "22"],
        }
    }

    /// 偵測這台電腦能不能用此編碼器：編一小段黑畫面，ffmpeg 結束碼為 0 就是可以。
    pub fn probe_args(self) -> Vec<String> {
        let mut args = strings(&[
            "-hide_banner", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=1280x720:d=0.2",
            "-frames:v", "3", "-c:v",
        ]);
        args.push(self.codec().to_string());
        args.extend(strings(&["-f", "null", "-"]));
        args
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderSpec {
    pub clips: Vec<Clip>,
    /// 閃白過場長度（秒）。
    pub transition: f64,
    pub width: u32,
    pub height: u32,
    /// ffmpeg 幀率寫法，例如 `30000/1001`（29.97）。
    pub fps: String,
    pub encoder: Encoder,
    pub output: String,
}

/// 每個過場在輸出影片上的起點。重疊式過場：第 i 個過場從
/// 「前 i 段長度總和 − i × 過場長度」開始。
pub fn transition_offsets(durations: &[f64], transition: f64) -> Vec<f64> {
    let mut offsets = Vec::new();
    let mut elapsed = 0.0;
    for (i, d) in durations.iter().enumerate().take(durations.len().saturating_sub(1)) {
        elapsed += d;
        offsets.push(elapsed - transition * (i + 1) as f64);
    }
    offsets
}

/// 輸出影片總長：各段相加，再扣掉重疊的過場。
pub fn output_duration(spec: &RenderSpec) -> f64 {
    let total: f64 = spec.clips.iter().map(Clip::used_duration).sum();
    total - spec.transition * spec.clips.len().saturating_sub(1) as f64
}

/// 組出整支 ffmpeg 指令（不含 ffmpeg 本身）。
///
/// 每段先統一成相同解析度、幀率、像素格式、聲音格式，再用 `xfade=fadewhite`
/// 和 `acrossfade` 串起來。沒有聲音的素材補一段等長靜音，避免 acrossfade 斷掉。
pub fn build_render_args(spec: &RenderSpec) -> Result<Vec<String>, String> {
    if spec.clips.is_empty() {
        return Err("沒有任何影片".to_string());
    }
    let t = spec.transition;
    if spec.clips.len() > 1 && t <= 0.0 {
        return Err("閃白長度必須大於 0 秒".to_string());
    }
    for (i, clip) in spec.clips.iter().enumerate() {
        let d = clip.used_duration();
        if d <= 0.0 {
            return Err(format!("第 {} 段影片長度為 0，請檢查裁切時間", i + 1));
        }
        if spec.clips.len() > 1 && d <= t * 2.0 {
            return Err(format!("第 {} 段影片（{d:.1} 秒）比閃白過場還短", i + 1));
        }
    }

    let mut args = strings(&["-hide_banner", "-y"]);
    for clip in &spec.clips {
        if clip.is_trimmed() {
            args.extend(["-ss".into(), secs(clip.trim_start()), "-t".into(), secs(clip.used_duration())]);
        }
        args.extend(["-i".into(), clip.path.clone()]);
    }

    let (w, h, fps) = (spec.width, spec.height, &spec.fps);
    let mut graph = Vec::new();
    for (i, clip) in spec.clips.iter().enumerate() {
        graph.push(format!(
            "[{i}:v]setpts=PTS-STARTPTS,scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps},format=yuv420p[v{i}]"
        ));
        if clip.has_audio {
            graph.push(format!(
                "[{i}:a]asetpts=PTS-STARTPTS,aresample=48000,\
                 aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}]"
            ));
        } else {
            graph.push(format!(
                "anullsrc=channel_layout=stereo:sample_rate=48000,atrim=0:{},aformat=sample_fmts=fltp[a{i}]",
                secs(clip.used_duration())
            ));
        }
    }

    let durations: Vec<f64> = spec.clips.iter().map(Clip::used_duration).collect();
    let offsets = transition_offsets(&durations, t);
    let (mut v_last, mut a_last) = ("v0".to_string(), "a0".to_string());
    for (k, offset) in offsets.iter().enumerate() {
        let next = k + 1;
        let (v_out, a_out) = (format!("vx{next}"), format!("ax{next}"));
        graph.push(format!(
            "[{v_last}][v{next}]xfade=transition=fadewhite:duration={}:offset={}[{v_out}]",
            secs(t),
            secs(*offset)
        ));
        graph.push(format!("[{a_last}][a{next}]acrossfade=d={}[{a_out}]", secs(t)));
        v_last = v_out;
        a_last = a_out;
    }

    args.push("-filter_complex".into());
    args.push(graph.join(";"));
    args.extend(["-map".into(), format!("[{v_last}]"), "-map".into(), format!("[{a_last}]")]);
    args.extend(["-c:v".into(), spec.encoder.codec().into()]);
    args.extend(strings(spec.encoder.quality_args()));
    args.extend(strings(&[
        "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart",
        "-progress", "pipe:1", "-nostats",
    ]));
    args.push(spec.output.clone());
    Ok(args)
}

/// ffmpeg `-progress pipe:1` 輸出的一行，回傳目前輸出到第幾秒。
pub fn parse_progress_line(line: &str) -> Option<f64> {
    // out_time_ms 其實也是微秒（ffmpeg 的歷史包袱），兩個都當微秒處理。
    let value = line
        .strip_prefix("out_time_us=")
        .or_else(|| line.strip_prefix("out_time_ms="))?;
    let us: i64 = value.trim().parse().ok()?;
    (us >= 0).then(|| us as f64 / 1_000_000.0)
}

/// ffprobe 讀到的影片資訊。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    /// 例如 29.97
    pub fps: f64,
    pub has_audio: bool,
}

pub fn probe_args(path: &str) -> Vec<String> {
    let mut args = strings(&["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]);
    args.push(path.to_string());
    args
}

/// 解析 `ffprobe -print_format json -show_format -show_streams` 的輸出。
pub fn parse_probe_json(json: &str) -> Result<ProbeInfo, String> {
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        streams: Vec<Stream>,
        format: Option<Format>,
    }
    #[derive(Deserialize)]
    struct Stream {
        codec_type: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        avg_frame_rate: Option<String>,
        r_frame_rate: Option<String>,
        duration: Option<String>,
        disposition: Option<Disposition>,
    }
    #[derive(Deserialize)]
    struct Disposition {
        #[serde(default)]
        attached_pic: u8,
    }
    #[derive(Deserialize)]
    struct Format {
        duration: Option<String>,
    }

    let root: Root = serde_json::from_str(json).map_err(|e| format!("讀不懂 ffprobe 的輸出：{e}"))?;
    // 封面圖（attached_pic）也被當成 video stream，要跳過
    let video = root
        .streams
        .iter()
        .find(|s| {
            s.codec_type.as_deref() == Some("video")
                && s.disposition.as_ref().map_or(0, |d| d.attached_pic) == 0
        })
        .ok_or("這個檔案裡沒有影像")?;
    let has_audio = root.streams.iter().any(|s| s.codec_type.as_deref() == Some("audio"));

    let duration = root
        .format
        .and_then(|f| f.duration)
        .or_else(|| video.duration.clone())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| *d > 0.0)
        .ok_or("讀不到影片長度")?;

    let fps = [&video.avg_frame_rate, &video.r_frame_rate]
        .into_iter()
        .flatten()
        .find_map(|r| parse_rational(r))
        .unwrap_or(0.0);

    Ok(ProbeInfo {
        duration,
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        fps,
        has_audio,
    })
}

fn parse_rational(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then(|| n / d)
}

/// 縮圖要截的時間點：平均分布，避開頭尾（頭尾常是黑畫面）。
pub fn thumbnail_times(duration: f64, count: usize) -> Vec<f64> {
    if duration <= 0.0 || count == 0 {
        return Vec::new();
    }
    let step = duration / count as f64;
    (0..count).map(|i| step * (i as f64 + 0.5)).collect()
}

/// 截一張縮圖（輸出到 stdout 的 JPEG）。`-ss` 放在 `-i` 前面是快速跳轉，長影片也只要零點幾秒。
pub fn thumbnail_args(path: &str, at: f64, height: u32) -> Vec<String> {
    let mut args = strings(&["-hide_banner", "-v", "error", "-ss"]);
    args.push(secs(at));
    args.extend(["-i".into(), path.to_string()]);
    args.extend(strings(&["-frames:v", "1", "-vf"]));
    args.push(format!("scale=-2:{height}"));
    args.extend(strings(&["-q:v", "6", "-f", "image2", "-c:v", "mjpeg", "pipe:1"]));
    args
}

/// yt-dlp 下載參數：最高 1080p、合併成 mp4，進度一行一行印出來方便解析，
/// 最後印出存檔路徑。
pub fn ytdlp_args(url: &str, output_template: &str, ffmpeg_dir: &str) -> Vec<String> {
    let mut args = strings(&[
        "-f",
        "bv*[height<=1080][ext=mp4]+ba[ext=m4a]/bv*[height<=1080]+ba/b[height<=1080]/b",
        "--merge-output-format",
        "mp4",
        "--no-playlist",
        "--newline",
        // --print 會默認 --simulate 和 --quiet，要明確關掉模擬、打開進度
        "--no-simulate",
        "--progress",
        "--progress-template",
        "download:PROGRESS %(progress._percent_str)s",
        "--print",
        "video:ID %(id)s",
        "--print",
        "after_move:FILEPATH %(filepath)s",
        "--ffmpeg-location",
    ]);
    args.push(ffmpeg_dir.to_string());
    args.extend(["-o".into(), output_template.to_string(), url.to_string()]);
    args
}

pub enum YtdlpLine {
    /// 0–100
    Progress(f64),
    /// 影片 id；已下載過時 yt-dlp 可能不印 FILEPATH，用 id 找檔案
    Id(String),
    FilePath(String),
}

pub fn parse_ytdlp_line(line: &str) -> Option<YtdlpLine> {
    if let Some(rest) = line.strip_prefix("PROGRESS ") {
        let pct: f64 = rest.trim().trim_end_matches('%').trim().parse().ok()?;
        return Some(YtdlpLine::Progress(pct));
    }
    if let Some(id) = line.strip_prefix("ID ") {
        return Some(YtdlpLine::Id(id.trim().to_string()));
    }
    line.strip_prefix("FILEPATH ").map(|p| YtdlpLine::FilePath(p.trim().to_string()))
}

/// 拿掉 Windows 檔名不允許的字元（全形標點可以用，不用換）。
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    // Windows 不允許結尾是空白或句點
    cleaned.trim().trim_end_matches(['.', ' ']).to_string()
}

fn secs(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".to_string() } else { s.to_string() }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(path: &str, duration: f64) -> Clip {
        Clip { path: path.into(), duration, has_audio: true, start: None, end: None }
    }

    fn spec(clips: Vec<Clip>) -> RenderSpec {
        RenderSpec {
            clips,
            transition: 0.5,
            width: 1920,
            height: 1080,
            fps: "30000/1001".into(),
            encoder: Encoder::X264,
            output: "out.mp4".into(),
        }
    }

    #[test]
    fn offsets_account_for_overlap() {
        assert_eq!(transition_offsets(&[60.0, 20.0, 45.0], 0.5), vec![59.5, 79.0]);
        assert!(transition_offsets(&[60.0], 0.5).is_empty());
    }

    #[test]
    fn output_duration_subtracts_transitions() {
        let s = spec(vec![clip("a", 60.0), clip("b", 20.0), clip("c", 45.0)]);
        assert_eq!(output_duration(&s), 124.0);
    }

    #[test]
    fn three_clip_graph_chains_xfade_and_acrossfade() {
        let s = spec(vec![clip("main.mp4", 60.0), clip("yt.mp4", 20.0), clip("promo.mp4", 45.0)]);
        let args = build_render_args(&s).unwrap();
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.contains("[v0][v1]xfade=transition=fadewhite:duration=0.5:offset=59.5[vx1]"));
        assert!(graph.contains("[vx1][v2]xfade=transition=fadewhite:duration=0.5:offset=79[vx2]"));
        assert!(graph.contains("[a0][a1]acrossfade=d=0.5[ax1]"));
        assert!(graph.contains("[ax1][a2]acrossfade=d=0.5[ax2]"));
        assert!(graph.contains("fps=30000/1001"));
        assert!(args.windows(2).any(|w| w == ["-map", "[vx2]"]));
        assert!(args.windows(2).any(|w| w == ["-map", "[ax2]"]));
        assert_eq!(args.last().unwrap(), "out.mp4");
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 3);
    }

    #[test]
    fn trimmed_clip_seeks_input_and_uses_trimmed_length() {
        let mut yt = clip("yt.mp4", 300.0);
        yt.start = Some(30.0);
        yt.end = Some(90.0);
        let s = spec(vec![clip("main.mp4", 60.0), yt, clip("promo.mp4", 20.0)]);
        let args = build_render_args(&s).unwrap();
        let i = args.iter().position(|a| a == "yt.mp4").unwrap();
        assert_eq!(&args[i - 5..i], ["-ss", "30", "-t", "60", "-i"]);
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.contains("offset=119[vx2]"), "{graph}");
    }

    #[test]
    fn silent_clip_gets_generated_silence() {
        let mut promo = clip("promo.mp4", 20.0);
        promo.has_audio = false;
        let args = build_render_args(&spec(vec![clip("main.mp4", 60.0), promo])).unwrap();
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.contains("anullsrc=channel_layout=stereo:sample_rate=48000,atrim=0:20"));
        assert!(!graph.contains("[1:a]"));
    }

    #[test]
    fn single_clip_is_just_normalized() {
        let args = build_render_args(&spec(vec![clip("main.mp4", 60.0)])).unwrap();
        assert!(args.windows(2).any(|w| w == ["-map", "[v0]"]));
        assert!(!args.iter().any(|a| a.contains("xfade")));
    }

    #[test]
    fn rejects_bad_specs() {
        assert!(build_render_args(&spec(vec![])).is_err());
        assert!(build_render_args(&spec(vec![clip("a", 60.0), clip("b", 0.8)])).is_err());
        let mut s = spec(vec![clip("a", 60.0), clip("b", 20.0)]);
        s.transition = 0.0;
        assert!(build_render_args(&s).is_err());
    }

    #[test]
    fn hardware_encoder_args() {
        let mut s = spec(vec![clip("a", 60.0), clip("b", 20.0)]);
        s.encoder = Encoder::Nvenc;
        let args = build_render_args(&s).unwrap();
        assert!(args.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
        assert!(Encoder::Qsv.probe_args().contains(&"h264_qsv".to_string()));
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress_line("out_time_us=59500000"), Some(59.5));
        assert_eq!(parse_progress_line("out_time_ms=1000000"), Some(1.0));
        assert_eq!(parse_progress_line("out_time_us=N/A"), None);
        assert_eq!(parse_progress_line("frame=10"), None);
    }

    #[test]
    fn probe_json() {
        let json = r#"{"streams":[
            {"codec_type":"video","width":1920,"height":1080,"avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001"},
            {"codec_type":"audio"},
            {"codec_type":"video","width":600,"height":600,"disposition":{"attached_pic":1}}
        ],"format":{"duration":"1935.402000"}}"#;
        let p = parse_probe_json(json).unwrap();
        assert_eq!((p.width, p.height), (1920, 1080));
        assert!((p.fps - 29.97).abs() < 0.01);
        assert!(p.has_audio);
        assert!((p.duration - 1935.402).abs() < 1e-6);
    }

    #[test]
    fn probe_json_without_video_fails() {
        assert!(parse_probe_json(r#"{"streams":[{"codec_type":"audio"}],"format":{"duration":"3"}}"#).is_err());
    }

    #[test]
    fn thumbnails_spread_evenly() {
        assert_eq!(thumbnail_times(100.0, 4), vec![12.5, 37.5, 62.5, 87.5]);
        assert!(thumbnail_times(0.0, 4).is_empty());
    }

    #[test]
    fn ytdlp_lines() {
        assert!(matches!(parse_ytdlp_line("PROGRESS  42.3%"), Some(YtdlpLine::Progress(p)) if (p - 42.3).abs() < 1e-9));
        assert!(matches!(parse_ytdlp_line("FILEPATH C:\\x\\abc.mp4"), Some(YtdlpLine::FilePath(p)) if p == "C:\\x\\abc.mp4"));
        assert!(matches!(parse_ytdlp_line("ID dQw4w9WgXcQ"), Some(YtdlpLine::Id(i)) if i == "dQw4w9WgXcQ"));
        assert!(parse_ytdlp_line("[youtube] abc: Downloading webpage").is_none());
        let args = ytdlp_args("https://youtu.be/x", "/tmp/%(id)s.%(ext)s", "/tools");
        assert!(args.contains(&"--no-simulate".to_string()));
        assert_eq!(args.last().unwrap(), "https://youtu.be/x");
    }

    #[test]
    fn file_names() {
        assert_eq!(
            sanitize_file_name("Intel「逆轉王牌」爆炸！？ 魏哲家嗆：食物【關鍵時刻】20261016-1張炤和"),
            "Intel「逆轉王牌」爆炸！？ 魏哲家嗆：食物【關鍵時刻】20261016-1張炤和"
        );
        assert_eq!(sanitize_file_name("a/b:c?. "), "a_b_c_");
    }
}
