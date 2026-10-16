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
    /// 輸出 open 以上的檔頭（日期標題、串一～串六、全集）。
    pub include_header: bool,
    /// 幾個字以內、沒有標點的單獨一行視為講者名字並刪除。
    pub max_speaker_chars: usize,
    /// 畫面配色 id（對應 styles.css 的 `[data-theme]`）。
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host: "張炤和".to_string(),
            program_tag: "【關鍵時刻】".to_string(),
            label_segments: true,
            number_line: false,
            include_header: false,
            max_speaker_chars: 6,
            theme: "light".to_string(),
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
        assert!(!s.include_header);
    }
}
