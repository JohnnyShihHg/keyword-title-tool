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
    /// 整體音量增益（dB），剪映的 +6 = 6.0
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
    }
}
