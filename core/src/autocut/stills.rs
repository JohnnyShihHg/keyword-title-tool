//! 靜圖 → 版型、來源字樣板、小灰階。吃已解碼的圖（讀檔在 app 端）。

use super::frame::{layout_from_strip, luma, small_gray, Mask, Strip};
use super::{Layout, H, ROI_DUAL, ROI_FULL, W};
use image::{imageops, GrayImage, RgbImage};

pub struct Still {
    /// 檔名（報告、快取簽名用）
    pub name: String,
    /// 完整路徑（出片時給 ffmpeg）
    pub path: String,
    pub layout: Layout,
    /// 屬於哪一個來源字樣板（沒有來源字＝None）
    pub template: Option<usize>,
    /// 80×45 灰階（比相似度用）
    pub small: GrayImage,
}

pub struct Template {
    pub layout: Layout,
    pub mask: Mask,
    pub stills: Vec<usize>,
}

#[derive(Default)]
pub struct StillSet {
    pub stills: Vec<Still>,
    pub templates: Vec<Template>,
}

impl StillSet {
    /// `(檔名, 路徑, 圖)`，依路徑排序後處理（樣板編號跟著順序走，順序要固定）
    pub fn build(mut images: Vec<(String, String, RgbImage)>) -> StillSet {
        images.sort_by(|a, b| a.1.cmp(&b.1));
        let mut stills = Vec::new();
        let mut templates: Vec<Template> = Vec::new();
        for (name, path, img) in images {
            let img = if img.dimensions() != (W, H) { imageops::resize(&img, W, H, imageops::FilterType::Triangle) } else { img };
            let lum = |x: u32, y: u32| luma(&img.get_pixel(x, y).0);
            let mut strip: Strip = [[0f32; 48]; 72];
            for (r, row) in strip.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v = (0..10).map(|k| lum(938 + c as u32, 90 + r as u32 * 10 + k)).sum::<f32>() / 10.0;
                }
            }
            let layout = layout_from_strip(&strip);
            let (rx, ry, rw, rh) = if layout == Layout::Full { ROI_FULL } else { ROI_DUAL };
            let m = Mask::from_fn(rw as usize, rh as usize, |x, y| lum(rx + x as u32, ry + y as u32));
            let idx = stills.len();
            let template = if m.count >= if layout == Layout::Full { 120 } else { 50 } {
                // 同一個來源字只留一份樣板，靜圖掛在它底下
                match templates.iter().position(|t| t.layout == layout && t.mask.iou(&m) >= 0.75) {
                    Some(k) => {
                        templates[k].stills.push(idx);
                        Some(k)
                    }
                    None => {
                        templates.push(Template { layout, mask: m, stills: vec![idx] });
                        Some(templates.len() - 1)
                    }
                }
            } else {
                None
            };
            let s320 = imageops::resize(&img, 320, 180, imageops::FilterType::Triangle);
            let small = small_gray(&s320, layout);
            stills.push(Still { name, path, layout, template, small });
        }
        StillSet { stills, templates }
    }

    /// 快取簽名：每張靜圖的檔名＋版型＋樣板編號。靜圖增減、換圖造成判斷改變，簽名就不同
    pub fn signature(&self) -> String {
        self.stills
            .iter()
            .map(|s| format!("{}|{}|{}", s.name, s.layout == Layout::Dual, s.template.map(|t| t as i64).unwrap_or(-1)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 報告開頭的靜圖清單
    pub fn report(&self) -> String {
        let mut r = format!("== 靜圖 {} 張、來源字樣板 {} 個 ==\n", self.stills.len(), self.templates.len());
        for s in &self.stills {
            let t = s.template.map(|k| format!("樣板 #{k}")).unwrap_or_else(|| "無來源字→比畫面".into());
            r.push_str(&format!("  {}  {}  {}\n", s.name, s.layout.zh(), t));
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 滿版靜圖：右上角來源字區畫一個白色字形（offset 讓字形不同）
    fn still_with_label(shape: u32) -> RgbImage {
        RgbImage::from_fn(W, H, |x, y| {
            let (lx, ly) = (x.wrapping_sub(ROI_FULL.0), y.wrapping_sub(ROI_FULL.1));
            let on = lx < 160 && ly < 80 && ((lx / 8 + ly / 8 * shape) % 3 == 0);
            if on {
                image::Rgb([240, 240, 240])
            } else {
                image::Rgb([((x / 7) % 90) as u8 + 20, 60, ((y / 5) % 80) as u8 + 20])
            }
        })
    }

    #[test]
    fn same_label_shares_template() {
        let a = still_with_label(1);
        let set = StillSet::build(vec![
            ("b.jpg".into(), "/s/b.jpg".into(), a.clone()),
            ("a.jpg".into(), "/s/a.jpg".into(), a),
            ("c.jpg".into(), "/s/c.jpg".into(), still_with_label(2)),
            ("d.jpg".into(), "/s/d.jpg".into(), RgbImage::from_pixel(W, H, image::Rgb([50, 50, 50]))),
        ]);
        let names: Vec<&str> = set.stills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a.jpg", "b.jpg", "c.jpg", "d.jpg"], "依路徑排序");
        assert!(set.stills.iter().all(|s| s.layout == Layout::Full));
        assert_eq!(set.stills[0].template, Some(0));
        assert_eq!(set.stills[1].template, Some(0));
        assert_eq!(set.stills[2].template, Some(1));
        assert_eq!(set.stills[3].template, None, "沒有來源字");
        assert_eq!(set.templates[0].stills, vec![0, 1]);
        assert!(set.signature().starts_with("a.jpg|false|0\nb.jpg|false|0"));
    }
}
