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
    /// 全集「砍掉片尾」時，第一、二、四、五段從檔尾砍掉的秒數（單集輸出接的 45 秒 promo）
    pub promo_tail_secs: f64,
    /// 輸出影片的資料夾；空白時每次另存新檔時選
    pub output_dir: String,
    /// `auto`（自動偵測硬體編碼）或指定 `x264` / `nvenc` / `qsv` / `amf`
    pub encoder: String,
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
            promo_tail_secs: 45.0,
            output_dir: String::new(),
            encoder: "auto".to_string(),
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
