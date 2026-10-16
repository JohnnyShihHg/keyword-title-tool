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
    /// 和前一段怎麼接（第一段忽略）。
    #[serde(default)]
    pub join: Join,
    /// [`ProbeInfo::copy_signature`]，判斷能不能走快速模式。
    #[serde(default)]
    pub signature: Option<String>,
}

/// 兩段之間的接法。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Join {
    /// 白閃過場（兩段重疊 `transition` 秒）
    #[default]
    Flash,
    /// 直接接（乾淨檔接 promo 用）
    Cut,
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
    /// Windows Media Foundation：透過顯示卡驅動做硬體編碼，NVIDIA／Intel／AMD 都能用。
    /// 畫質控制不如各家專用編碼器細，所以排在最後當備援。
    Mf,
}

impl Encoder {
    /// 自動偵測時依序嘗試的硬體編碼器（都失敗就用 x264）。
    pub const HARDWARE: [Encoder; 4] = [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::Mf];

    fn codec(self) -> &'static str {
        match self {
            Encoder::X264 => "libx264",
            Encoder::Nvenc => "h264_nvenc",
            Encoder::Qsv => "h264_qsv",
            Encoder::Amf => "h264_amf",
            Encoder::Mf => "h264_mf",
        }
    }

    /// 編碼器吃的像素格式：Intel QSV 和 Media Foundation 只收 nv12。
    fn pix_fmt(self) -> &'static str {
        match self {
            Encoder::Qsv | Encoder::Mf => "nv12",
            _ => "yuv420p",
        }
    }

    /// 顯示用名稱
    pub fn label(self) -> &'static str {
        match self {
            Encoder::X264 => "CPU",
            Encoder::Nvenc => "NVIDIA",
            Encoder::Qsv => "Intel",
            Encoder::Amf => "AMD",
            Encoder::Mf => "Windows 硬體加速",
        }
    }

    /// 畫質參數：都調在「肉眼看不出差異、檔案不過大」的程度。
    fn quality_args(self) -> &'static [&'static str] {
        match self {
            Encoder::X264 => &["-preset", "veryfast", "-crf", "20"],
            Encoder::Nvenc => &["-preset", "p4", "-rc", "vbr", "-cq", "21", "-b:v", "0"],
            Encoder::Qsv => &["-preset", "medium", "-global_quality", "21"],
            Encoder::Amf => &["-quality", "balanced", "-rc", "cqp", "-qp_i", "20", "-qp_p", "22"],
            // hw_encoding=1：只接受硬體編碼，沒有就失敗（不要默默退回 Windows 的軟體編碼）
            Encoder::Mf => &["-hw_encoding", "1", "-b:v", "12M"],
        }
    }

    /// 偵測這台電腦能不能用此編碼器：用輸出時同一組參數編一小段 1080p 黑畫面，
    /// ffmpeg 結束碼為 0 就是可以。失敗原因在 stderr（`-v error` 只留錯誤）。
    pub fn probe_args(self) -> Vec<String> {
        let mut args = strings(&[
            "-hide_banner", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=1920x1080:r=30000/1001:d=0.5",
            "-frames:v", "10", "-pix_fmt",
        ]);
        args.push(self.pix_fmt().to_string());
        args.extend(["-c:v".into(), self.codec().to_string()]);
        args.extend(strings(self.quality_args()));
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
    /// 整體音量增益（dB），剪映的 +6 就是 6.0；0 表示不調整。
    #[serde(default)]
    pub gain_db: f64,
}

/// 每段在成品上的位置。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placement {
    /// 和前一段的過場從第幾秒開始（只有閃白接法才有）
    pub transition_at: Option<f64>,
    /// 這段完全出現的時間點（閃白結束後）——章節時間就用這個
    pub visible_at: f64,
}

/// 依序算出每段在成品上的位置與成品總長。
///
/// 閃白接法兩段重疊 `transition` 秒：過場從「目前長度 − transition」開始，
/// 結束時這段才完全出現；直接接就是接在目前長度。
pub fn plan(spec: &RenderSpec) -> (Vec<Placement>, f64) {
    let t = spec.transition;
    let mut length = 0.0;
    let mut placements = Vec::with_capacity(spec.clips.len());
    for (i, clip) in spec.clips.iter().enumerate() {
        let d = clip.used_duration();
        if i == 0 {
            placements.push(Placement { transition_at: None, visible_at: 0.0 });
            length = d;
        } else if clip.join == Join::Flash {
            let at = length - t;
            placements.push(Placement { transition_at: Some(at), visible_at: at + t });
            length += d - t;
        } else {
            placements.push(Placement { transition_at: None, visible_at: length });
            length += d;
        }
    }
    (placements, length)
}

/// 輸出影片總長：各段相加，再扣掉閃白重疊的部分。
pub fn output_duration(spec: &RenderSpec) -> f64 {
    plan(spec).1
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
    let has_flash = spec.clips.iter().skip(1).any(|c| c.join == Join::Flash);
    if has_flash && t <= 0.0 {
        return Err("閃白長度必須大於 0 秒".to_string());
    }
    for (i, clip) in spec.clips.iter().enumerate() {
        let d = clip.used_duration();
        if d <= 0.0 {
            return Err(format!("第 {} 段影片長度為 0，請檢查裁切時間", i + 1));
        }
        if has_flash && d <= t * 2.0 {
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
    // settb=AVTB：concat 的輸出時間基準是 1/1000000，xfade 要求兩個輸入的時間基準相同，
    // 所以每段都先統一成同一個時間基準，閃白和直接接才能混著用。
    for (i, clip) in spec.clips.iter().enumerate() {
        graph.push(format!(
            "[{i}:v]setpts=PTS-STARTPTS,scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps},format=yuv420p,settb=AVTB[v{i}]"
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

    // 逐段往後接：閃白用 xfade + acrossfade（重疊），直接接用 concat。
    // xfade 的 offset 是相對於「目前已接好的那條」的長度，所以邊接邊累計。
    let (placements, _) = plan(spec);
    let (mut v_last, mut a_last) = ("v0".to_string(), "a0".to_string());
    for (k, (clip, place)) in spec.clips.iter().zip(&placements).enumerate().skip(1) {
        let (v_out, a_out) = (format!("vx{k}"), format!("ax{k}"));
        match (clip.join, place.transition_at) {
            (Join::Flash, Some(at)) => {
                graph.push(format!(
                    "[{v_last}][v{k}]xfade=transition=fadewhite:duration={}:offset={}[{v_out}]",
                    secs(t),
                    secs(at)
                ));
                graph.push(format!("[{a_last}][a{k}]acrossfade=d={}[{a_out}]", secs(t)));
            }
            _ => {
                graph.push(format!("[{v_last}][v{k}]concat=n=2:v=1:a=0[{v_out}]"));
                graph.push(format!("[{a_last}][a{k}]concat=n=2:v=0:a=1[{a_out}]"));
            }
        }
        v_last = v_out;
        a_last = a_out;
    }

    // 整體音量：先拉高，再用限幅器壓住峰值，避免原本就大聲的地方爆音。
    // level=false 關掉 alimiter 的自動音量，不然它會再把整體推到上限。
    if spec.gain_db != 0.0 {
        graph.push(format!(
            "[{a_last}]volume={}dB,alimiter=limit=0.95:level=false[again]",
            secs(spec.gain_db)
        ));
        a_last = "again".to_string();
    }

    args.push("-filter_complex".into());
    args.push(graph.join(";"));
    args.extend(["-map".into(), format!("[{v_last}]"), "-map".into(), format!("[{a_last}]")]);
    args.extend(["-c:v".into(), spec.encoder.codec().into()]);
    args.extend(strings(spec.encoder.quality_args()));
    args.extend(["-pix_fmt".into(), spec.encoder.pix_fmt().into()]);
    args.extend(strings(&[
        "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart",
        "-progress", "pipe:1", "-nostats",
    ]));
    args.push(spec.output.clone());
    Ok(args)
}

/// 快速模式：全部直接接（沒有閃白）、沒有裁切、每段規格指紋相同且是目標解析度時，
/// 畫面不用重新編碼，用 concat demuxer 直接複製串接，只重新編碼聲音（為了音量增益）。
/// 30 分鐘的影片從十幾分鐘縮短到一分鐘內，畫質也完全不損失。
pub fn can_copy_video(spec: &RenderSpec) -> bool {
    let Some(first) = spec.clips.first().and_then(|c| c.signature.as_deref()) else {
        return false;
    };
    first.contains(&format!("|{}x{}|", spec.width, spec.height))
        && spec.clips.iter().skip(1).all(|c| c.join == Join::Cut)
        && spec.clips.iter().all(|c| {
            c.has_audio && !c.is_trimmed() && c.signature.as_deref() == Some(first)
        })
}

/// concat demuxer 的清單檔內容（ffconcat 格式）。
pub fn concat_list(spec: &RenderSpec) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    for clip in &spec.clips {
        // 單引號內全部照字面解讀；路徑本身的單引號要跳出來另外處理
        out.push_str(&format!("file '{}'\n", clip.path.replace('\'', "'\\''")));
    }
    out
}

/// 快速模式的 ffmpeg 參數，`list_path` 是 [`concat_list`] 寫成的檔案。
pub fn build_copy_args(spec: &RenderSpec, list_path: &str) -> Vec<String> {
    let mut args = strings(&["-hide_banner", "-y", "-f", "concat", "-safe", "0", "-i"]);
    args.push(list_path.to_string());
    args.extend(strings(&["-map", "0:v:0", "-map", "0:a:0", "-c:v", "copy"]));
    if spec.gain_db != 0.0 {
        args.push("-af".into());
        args.push(format!("volume={}dB,alimiter=limit=0.95:level=false", secs(spec.gain_db)));
        args.extend(strings(&["-c:a", "aac", "-b:a", "192k"]));
    } else {
        args.extend(strings(&["-c:a", "copy"]));
    }
    args.extend(strings(&["-movflags", "+faststart", "-progress", "pipe:1", "-nostats"]));
    args.push(spec.output.clone());
    args
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
    /// 影音規格指紋（編碼、profile、解析度、像素格式、幀率、聲音格式）。
    /// 每段指紋都一樣時，畫面可以直接複製串接，不用重新編碼。
    pub copy_signature: String,
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
        codec_name: Option<String>,
        profile: Option<String>,
        pix_fmt: Option<String>,
        sample_rate: Option<String>,
        channels: Option<u32>,
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

    let audio = root.streams.iter().find(|s| s.codec_type.as_deref() == Some("audio"));
    let opt = |v: &Option<String>| v.clone().unwrap_or_default();
    let copy_signature = format!(
        "{}|{}|{}x{}|{}|{}|{}",
        opt(&video.codec_name),
        opt(&video.profile),
        video.width.unwrap_or(0),
        video.height.unwrap_or(0),
        opt(&video.pix_fmt),
        opt(&video.r_frame_rate),
        audio.map_or("noaudio".to_string(), |a| format!(
            "{}|{}|{}",
            opt(&a.codec_name),
            opt(&a.sample_rate),
            a.channels.unwrap_or(0)
        )),
    );

    Ok(ProbeInfo {
        duration,
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        fps,
        has_audio,
        copy_signature,
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
        Clip {
            path: path.into(),
            duration,
            has_audio: true,
            start: None,
            end: None,
            join: Join::Flash,
            signature: Some("h264|High|1920x1080|yuv420p|30000/1001|aac|48000|2".into()),
        }
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
            gain_db: 0.0,
        }
    }

    #[test]
    fn plan_places_flash_and_cut_joins() {
        // 全集：第一段 ｜ promo20 ⚡ 第二段 ⚡ 第三段 ｜ promo45
        let mut clips = vec![clip("s1", 600.0), clip("p20", 20.0), clip("s2", 300.0), clip("s3", 400.0), clip("p45", 45.0)];
        clips[1].join = Join::Cut;
        clips[4].join = Join::Cut;
        let (places, total) = plan(&spec(clips));
        let visible: Vec<f64> = places.iter().map(|p| p.visible_at).collect();
        assert_eq!(visible, vec![0.0, 600.0, 620.0, 919.5, 1319.0]);
        assert_eq!(places[2].transition_at, Some(619.5));
        assert_eq!(places[1].transition_at, None);
        assert_eq!(total, 1364.0);
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
    fn cut_join_uses_concat_and_shifts_later_offsets() {
        // 單集：完整影片 ⚡ YT ｜ promo45
        let mut promo = clip("promo45.mp4", 45.0);
        promo.join = Join::Cut;
        let s = spec(vec![clip("main.mp4", 60.0), clip("yt.mp4", 20.0), promo]);
        let args = build_render_args(&s).unwrap();
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.contains("[v0][v1]xfade=transition=fadewhite:duration=0.5:offset=59.5[vx1]"));
        assert!(graph.contains("[vx1][v2]concat=n=2:v=1:a=0[vx2]"));
        assert!(graph.contains("[ax1][a2]concat=n=2:v=0:a=1[ax2]"));
        assert_eq!(output_duration(&s), 124.5);
    }

    #[test]
    fn all_cut_joins_need_no_transition_length() {
        let mut b = clip("b", 0.8);
        b.join = Join::Cut;
        let mut s = spec(vec![clip("a", 60.0), b]);
        s.transition = 0.0;
        assert!(build_render_args(&s).is_ok());
    }

    #[test]
    fn gain_adds_volume_and_limiter() {
        let mut s = spec(vec![clip("a", 60.0), clip("b", 20.0)]);
        s.gain_db = 6.0;
        let args = build_render_args(&s).unwrap();
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.contains("[ax1]volume=6dB,alimiter=limit=0.95:level=false[again]"));
        assert!(args.windows(2).any(|w| w == ["-map", "[again]"]));
        s.gain_db = 0.0;
        assert!(!build_render_args(&s).unwrap().iter().any(|a| a.contains("volume=")));
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

        s.encoder = Encoder::Qsv;
        let args = build_render_args(&s).unwrap();
        assert!(args.windows(2).any(|w| w == ["-pix_fmt", "nv12"]));

        let mf = Encoder::Mf.probe_args();
        assert!(mf.windows(2).any(|w| w == ["-c:v", "h264_mf"]));
        assert!(mf.windows(2).any(|w| w == ["-hw_encoding", "1"]));
        assert_eq!(Encoder::HARDWARE.last(), Some(&Encoder::Mf));
    }

    #[test]
    fn copy_mode_only_when_all_cut_untrimmed_and_identical() {
        let mut promo = clip("promo45.mp4", 45.0);
        promo.join = Join::Cut;
        let s = spec(vec![clip("main.mp4", 1800.0), promo.clone()]);
        assert!(can_copy_video(&s));

        // 有閃白（接 YT）就不行
        let s2 = spec(vec![clip("main.mp4", 1800.0), clip("yt.mp4", 60.0), promo.clone()]);
        assert!(!can_copy_video(&s2));

        // 規格不同就不行
        let mut other = promo.clone();
        other.signature = Some("h264|Main|1920x1080|yuv420p|30/1|aac|44100|2".into());
        assert!(!can_copy_video(&spec(vec![clip("main.mp4", 1800.0), other])));

        // 不是 1080p 就不行（輸出規格要 1920×1080）
        let mut small = clip("main.mp4", 1800.0);
        small.signature = Some("h264|High|1280x720|yuv420p|30000/1001|aac|48000|2".into());
        let mut small_promo = promo.clone();
        small_promo.signature = small.signature.clone();
        assert!(!can_copy_video(&spec(vec![small, small_promo])));

        // 沒有聲音、或有裁切就不行
        let mut silent = promo.clone();
        silent.has_audio = false;
        assert!(!can_copy_video(&spec(vec![clip("main.mp4", 1800.0), silent])));
        let mut trimmed = promo;
        trimmed.end = Some(10.0);
        assert!(!can_copy_video(&spec(vec![clip("main.mp4", 1800.0), trimmed])));
    }

    #[test]
    fn copy_args_and_list() {
        let mut promo = clip("C:\\promo\\it's.mp4", 45.0);
        promo.join = Join::Cut;
        let mut s = spec(vec![clip("C:\\video\\main.mp4", 1800.0), promo]);
        s.gain_db = 6.0;
        assert_eq!(
            concat_list(&s),
            "ffconcat version 1.0\nfile 'C:\\video\\main.mp4'\nfile 'C:\\promo\\it'\\''s.mp4'\n"
        );
        let args = build_copy_args(&s, "list.txt");
        assert!(args.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(args.windows(2).any(|w| w == ["-af", "volume=6dB,alimiter=limit=0.95:level=false"]));
        assert_eq!(args.last().unwrap(), "out.mp4");
        s.gain_db = 0.0;
        assert!(build_copy_args(&s, "list.txt").windows(2).any(|w| w == ["-c:a", "copy"]));
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
            {"codec_type":"video","codec_name":"h264","profile":"High","pix_fmt":"yuv420p","width":1920,"height":1080,"avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001"},
            {"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2},
            {"codec_type":"video","width":600,"height":600,"disposition":{"attached_pic":1}}
        ],"format":{"duration":"1935.402000"}}"#;
        let p = parse_probe_json(json).unwrap();
        assert_eq!((p.width, p.height), (1920, 1080));
        assert!((p.fps - 29.97).abs() < 0.01);
        assert!(p.has_audio);
        assert!((p.duration - 1935.402).abs() < 1e-6);
        assert_eq!(p.copy_signature, "h264|High|1920x1080|yuv420p|30000/1001|aac|48000|2");
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
