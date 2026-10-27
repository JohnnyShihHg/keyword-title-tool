//! 自動切段：關鍵時刻側錄檔 → 遮好、切好的第 1～6 段單片＋全集。
//!
//! 從桌面工具 mask-detector 搬過來，判斷規則與常數照搬（用 10/20～10/22 三天側錄驗證過，
//! 改動前先用三天資料重跑比對）。這裡全是純函式：吃 ffmpeg 解出來的畫布、已解碼的靜圖、
//! OCR 文字，吐出每格判斷、段落與遮罩規劃。執行 ffmpeg／OCR、監看資料夾在 app 端。
//!
//! - 節目／廣告：節目畫面下方標題條上下各有一條深藍邊線（y≈870、y≈960）。
//! - 分段：每 2 秒一張標題條 OCR，跟文稿各段的子標題比對。
//! - 版型：雙框兩框之間的縫有固定的「暗亮暗」起伏。
//! - 要不要遮：靜圖右上角的來源字當樣板比對；或畫面跟某張靜圖夠像。
//! - 閃光的格子（亮度突然大變）不參與判斷，沿用前一格的結果。

pub mod bars;
pub mod cache;
pub mod frame;
pub mod plan;
pub mod stills;
pub mod titles;

pub use image;
use serde::Serialize;

/// 側錄解析度（1080i、29.97、上場優先）
pub const W: u32 = 1920;
pub const H: u32 = 1080;
pub const FPS: f64 = 30000.0 / 1001.0;

/// 雙框左框（影片區）x0 y0 x1 y1；遮罩實際只蓋到 BAND.0（不壓字幕）
pub const BOX: (u32, u32, u32, u32) = (104, 66, 942, 844);
/// 滿版遮罩要露出的橫帶（字幕＋標題條）y0 y1
pub const BAND: (u32, u32) = (770, 965);
/// 來源字範圍 x y w h
pub const ROI_FULL: (u32, u32, u32, u32) = (1570, 100, 180, 90);
pub const ROI_DUAL: (u32, u32, u32, u32) = (815, 115, 110, 75);
/// 標題條文字（OCR 用）x y w h
pub const OCR_BOX: (u32, u32, u32, u32) = (410, 868, 1480, 90);
pub const LABEL_IOU: f32 = 0.45;
pub const LABEL_IOU_STRONG: f32 = 0.62;
pub const SIM_NCC: f32 = 0.80;
pub const CUT_DIFF: f32 = 28.0;
pub const FLASH_LUMA: f32 = 25.0;
/// 段落結尾比切進廣告轉場早幾格（對照 1022 人工版平均）
pub const END_TRIM: usize = 6;
/// 段落開頭比 Logo 轉場結束晚幾格：判定結束那格還有淡淡的 Logo 殘影，下一格就是乾淨的棚內畫面。
/// （以前用 1022 人工版平均的 13 格，1026 實測主持人 Logo 後 0.25 秒就開口，會切掉第一個字）
pub const START_DELAY: usize = 1;

// 分析用畫布：ffmpeg 每格輸出一張 564×290 rgb24，把需要的區塊拼在一起
pub const CW: usize = 564;
pub const CH: usize = 290;
pub const CANVAS_BYTES: usize = CW * CH * 3;
/// 標題條區 y 860..970、x 400..1900 → 300×110
pub const BAR: (usize, usize, usize) = (0, 180, 300);
/// 滿版來源字 180×90（原尺寸）
pub const LF: (usize, usize) = (320, 0);
/// 雙框來源字 110×75（原尺寸）
pub const LD: (usize, usize) = (320, 90);
/// 兩框之間的縫 x 938..986、y 90..810 → 48×72
pub const STRIP: (usize, usize) = (516, 0);
/// Logo 轉場樣板、段落開頭比對用的小灰階尺寸
pub const LOGO_W: u32 = 80;
pub const LOGO_H: u32 = 45;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum Layout {
    Full,
    Dual,
}

impl Layout {
    pub fn zh(self) -> &'static str {
        if self == Layout::Full {
            "滿版"
        } else {
            "雙框"
        }
    }
}

/// 一格的判斷結果（掃描快取存的就是這個）
#[derive(Clone, Debug, PartialEq)]
pub struct FrameInfo {
    pub program: bool,
    pub layout: Layout,
    pub luma: f32,
    pub cut_diff: f32,
    /// 命中的來源字樣板與 IoU（-1＝沒有）
    pub tmpl: i16,
    pub iou: f32,
    /// 跟每張靜圖的相似度（不同版型＝0）
    pub sims: Vec<f32>,
}

/// 掃描用的 ffmpeg filter_complex：輸出 `[out]`（564×290 rgb24 畫布），
/// `ocr` 為 true 時另外輸出 `[ocr]`：每 2 秒一張標題條截圖。
pub fn scan_filter(ocr: bool) -> String {
    let split = if ocr { 6 } else { 5 };
    let mut f = format!(
        "[0:v]split={split}[a][b][c][d][e]{};\
         [a]scale=320:180[s];\
         [b]crop=1500:110:400:860,scale=300:110[bar];\
         [c]crop={}:{}:{}:{}[lf];\
         [d]crop={}:{}:{}:{}[ld];\
         [e]crop=48:720:938:90,scale=48:72:flags=area[sg];",
        if ocr { "[o]" } else { "" },
        ROI_FULL.2,
        ROI_FULL.3,
        ROI_FULL.0,
        ROI_FULL.1,
        ROI_DUAL.2,
        ROI_DUAL.3,
        ROI_DUAL.0,
        ROI_DUAL.1,
    );
    if ocr {
        f.push_str(&format!("[o]fps=1/2,crop={}:{}:{}:{}[ocr];", OCR_BOX.2, OCR_BOX.3, OCR_BOX.0, OCR_BOX.1));
    }
    f.push_str("[s][bar][lf][ld][sg]xstack=inputs=5:layout=0_0|0_180|320_0|320_90|516_0:fill=black,format=rgb24[out]");
    f
}

/// 秒 → `00:00:00.000`
pub fn tc(sec: f64) -> String {
    let ms = (sec.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// 格 → 秒
pub fn secs(frame: usize) -> f64 {
    frame as f64 / FPS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode() {
        assert_eq!(tc(0.0), "00:00:00.000");
        assert_eq!(tc(3723.5), "01:02:03.500");
        assert_eq!(tc(-1.0), "00:00:00.000");
    }

    #[test]
    fn filter_with_and_without_ocr() {
        let a = scan_filter(true);
        assert!(a.contains("split=6[a][b][c][d][e][o]"));
        assert!(a.contains("[o]fps=1/2,crop=1480:90:410:868[ocr]"));
        let b = scan_filter(false);
        assert!(b.contains("split=5[a][b][c][d][e];"));
        assert!(!b.contains("[ocr]"));
        assert!(b.ends_with("[out]"));
    }
}
