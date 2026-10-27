use serde::{Deserialize, Serialize};

/// 設定後台可調整的項目。存檔時缺少的欄位一律用預設值補，
/// 新版加欄位不會讓舊設定檔讀不進來。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 標題行最後的主持人名字。
    pub host: String,
    /// 節目名稱，接在段標題後面。
    pub program_tag: String,
    /// 每段開頭標註「第一段」「第二段」（第一段取代範本的 `open`）。
    pub label_segments: bool,
    /// 段標題下方輸出範本的「1.」「2.」編號行。
    pub number_line: bool,
    /// 全集區的時間點用半形「(00:27:08)」，YouTube 才能點擊跳轉；關掉就是範本的全形寫法。
    pub halfwidth_timestamps: bool,
    /// 輸出檔頭（日期標題、串一～串六、全集，後面接 YouTube 連結）。
    /// 舊版叫 include_header 且預設關閉；改名讓舊設定檔裡的 false 不會把它蓋掉。
    pub show_header: bool,
    /// 幾個字以內、沒有標點的單獨一行視為講者名字並刪除。
    pub max_speaker_chars: usize,
    /// 畫面配色 id（對應 styles.css 的 `[data-theme]`）。
    pub theme: String,

    // --- 影片 ---
    /// 20 秒版 promo 檔案路徑
    pub promo_20_path: String,
    /// 45 秒版 promo 檔案路徑
    pub promo_45_path: String,
    /// 單集「影片」積木預設的音量增益（dB），剪映的 +6 = 6.0；YT、promo、全集不加
    pub audio_gain_db: f64,
    /// 閃白過場長度（秒）
    pub transition_secs: f64,
    /// 全集「砍掉片尾」的微調秒數。第一、二、四、五段砍掉的長度＝45 秒 promo 檔的實際長度＋這個值
    /// （正數多砍、負數少砍）。
    pub promo_tail_adjust_secs: f64,
    /// 閃白判斷完成、轉檔完成時跳出提示框
    pub notify_popup: bool,
    /// 提示框出現時播提示音
    pub notify_sound: bool,
    /// 提示音種類（app 端合成，見 video.js 的 CHIMES）
    pub notify_sound_id: String,
    /// 提示音音量 0–100
    pub notify_volume: f64,
    /// 輸出影片的資料夾；空白時每次另存新檔時選
    pub output_dir: String,
    /// `auto`（自動偵測硬體編碼）或指定 `x264` / `nvenc` / `qsv` / `amf`
    pub encoder: String,

    // --- 共用資料夾同步 ---
    /// 總開關：開著才會自動載入／寫入當天資料夾的 txt
    pub sync_enabled: bool,
    /// 共用資料夾根目錄，例如 `\\數媒yt上傳\yt上傳`
    pub sync_root: String,
    /// 當天資料夾名稱，`{MMDD}` 等代入日期（見 `sync::folder_name`），例如 `{MMDD}關鍵` → `1016關鍵`
    pub sync_folder_format: String,
    /// 開啟時、別人改過時自動載入
    pub sync_auto_load: bool,
    /// 修改後自動寫回（txt 已經存在時；第一次要使用者自己按寫入或存成 txt）
    pub sync_auto_write: bool,

    // --- 自動切段 ---
    /// 側錄資料夾（每 10 分鐘一個檔）
    pub autocut_watch_dir: String,
    /// 要遮的靜圖資料夾；空白＝跟側錄同資料夾
    pub autocut_stills_dir: String,
    /// 成品資料夾；空白＝沿用 output_dir
    pub autocut_output_dir: String,
    /// 偵測到完整一段就自動遮蓋出片
    pub autocut_auto_render: bool,
    /// 只看這個時間（`21:50`）之後的側錄檔（側錄資料夾是整天的頻道錄影）；空白＝全部
    pub autocut_since: String,
    /// AI 補判（規則沒把握的鏡頭交給 Gemini 看右上角來源字）的 API 金鑰；空白＝不用 AI
    pub autocut_ai_key: String,
    /// AI 補判用的模型
    pub autocut_ai_model: String,
    /// 自動切段正片要壓的台標（1920×1080 透明 PNG，整張蓋上）；空白＝不壓。YT、promo 不壓
    pub autocut_watermark: String,
    /// 月份標（例如 `202610.png`）。檔名是年月時，出片自動換成節目日期那個月的同資料夾檔案
    pub autocut_watermark_month: String,

    // --- YouTube 上傳 ---
    /// Google Cloud「電腦版應用程式」OAuth 用戶端 ID；空白＝不能上傳
    pub yt_client_id: String,
    /// 同一個 OAuth 用戶端的密碼
    pub yt_client_secret: String,
    /// 每段都會加上的預設標記（逗號分隔；接在該段自己填的標記後面）
    pub yt_default_tags: String,
    /// 接在每段說明最後的固定內容（API 上傳不會套用 Studio 的「上傳預設設定」）
    pub yt_description_footer: String,
    /// 預設加入的播放清單 ID（可以多個，逗號分隔）；空白＝不加。每段可以在自動切段頁各自改
    pub yt_playlist_id: String,
    /// 自動切段每段成品（有串接就是串接成品）做好就自動上傳成私人影片
    pub yt_auto_upload: bool,
    /// 自動上傳前要等縮圖（共用資料夾的 N.jpg 或自己選的）
    pub yt_require_thumbnail: bool,
    /// 自動上傳前要等文稿有這段的後臺關鍵字
    pub yt_require_keywords: bool,
    /// 自動上傳前要等必選的播放清單都在頻道上建好了
    pub yt_require_playlist: bool,
    /// 每段一定要加的播放清單名稱，一行一個；`{日期}` 換成文稿日期（20261026）
    pub yt_required_playlists: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host: "張炤和".to_string(),
            program_tag: "【關鍵時刻】".to_string(),
            label_segments: true,
            number_line: false,
            show_header: true,
            halfwidth_timestamps: true,
            max_speaker_chars: 6,
            theme: "light".to_string(),
            promo_20_path: String::new(),
            promo_45_path: String::new(),
            audio_gain_db: 6.0,
            transition_secs: 0.5,
            promo_tail_adjust_secs: 0.0,
            notify_popup: true,
            notify_sound: true,
            notify_sound_id: "chime".to_string(),
            notify_volume: 60.0,
            output_dir: String::new(),
            encoder: "auto".to_string(),
            sync_enabled: false,
            sync_root: r"\\數媒yt上傳\yt上傳".to_string(),
            sync_folder_format: "{MMDD}關鍵".to_string(),
            sync_auto_load: true,
            sync_auto_write: true,
            autocut_watch_dir: String::new(),
            autocut_stills_dir: String::new(),
            autocut_output_dir: String::new(),
            autocut_auto_render: true,
            autocut_since: "21:50".to_string(),
            autocut_ai_key: String::new(),
            autocut_ai_model: "gemini-3.5-flash-lite".to_string(),
            autocut_watermark: String::new(),
            autocut_watermark_month: String::new(),
            yt_client_id: String::new(),
            yt_client_secret: String::new(),
            yt_default_tags: String::new(),
            yt_description_footer: String::new(),
            yt_playlist_id: String::new(),
            yt_auto_upload: false,
            yt_require_thumbnail: true,
            yt_require_keywords: true,
            yt_require_playlist: true,
            yt_required_playlists: "✨關鍵時刻【最新上傳】\n關鍵時刻{日期}全集".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // 模擬一份只存了 host 的舊設定檔（core 不依賴 serde_json，用 serde 內建的 map 反序列化器）。
        use serde::de::value::{Error, MapDeserializer};
        let map = MapDeserializer::<_, Error>::new(std::iter::once(("host", "別人")));
        let s = Settings::deserialize(map).unwrap();
        assert_eq!(s.host, "別人");
        assert_eq!(s.program_tag, "【關鍵時刻】");
        assert!(s.label_segments);
        assert!(s.show_header);
        assert_eq!(s.autocut_watch_dir, "");
        assert_eq!(s.autocut_stills_dir, "");
        assert_eq!(s.autocut_output_dir, "");
        assert!(s.autocut_auto_render);
        assert_eq!(s.autocut_ai_key, "", "舊設定檔沒有金鑰＝不用 AI");
        assert_eq!(s.autocut_ai_model, "gemini-3.5-flash-lite");
        assert_eq!(s.autocut_watermark, "", "舊設定檔沒有水印＝不壓");
    }
}
