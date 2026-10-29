//! 靜圖 → 版型、來源字樣板、小灰階。吃已解碼的圖（讀檔在 app 端）。

use super::frame::{layout_from_strip, luma, small_gray, Mask, Strip};
use super::{Layout, H, ROI_DUAL, ROI_FULL, W};
use image::{imageops, GrayImage, RgbImage};

#[derive(Clone)]
pub struct Still {
    /// 顯示名稱（報告用；段落子資料夾裡的會帶「第N段/」）
    pub name: String,
    /// 完整路徑（出片時給 ffmpeg）
    pub path: String,
    /// 放在靜圖資料夾的「第N段」子資料夾＝只給第 N 段用；直接放在資料夾裡＝每段都比
    pub segment: Option<usize>,
    pub layout: Layout,
    /// 屬於哪一個來源字樣板（沒有來源字＝None）
    pub template: Option<usize>,
    /// 80×45 灰階（比相似度用）
    pub small: GrayImage,
}

#[derive(Clone)]
pub struct Template {
    pub layout: Layout,
    pub mask: Mask,
    pub stills: Vec<usize>,
    /// 只有一行（「翻攝 X」「翻攝 微博」沒有說明）：別的素材常常一模一樣，字形像不能當證據
    pub generic: bool,
}

#[derive(Clone, Default)]
pub struct StillSet {
    pub stills: Vec<Still>,
    pub templates: Vec<Template>,
    /// `for_segment` 排除掉的靜圖（別段的）；空的＝全部可用
    pub off: Vec<bool>,
}

/// 檔名（不含資料夾）
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// 段落子資料夾名稱：`第2段`、`第二段` → 2
pub fn segment_of_dir(dir: &str) -> Option<usize> {
    let n = dir.strip_prefix('第')?.strip_suffix('段')?;
    n.parse().ok().or_else(|| ["一", "二", "三", "四", "五", "六", "七", "八", "九"].iter().position(|c| *c == n).map(|k| k + 1))
}

/// 路徑的上一層是段落子資料夾 → 第幾段
pub fn segment_of_path(path: &str) -> Option<usize> {
    let mut parts = path.rsplit(['/', '\\']);
    parts.next()?;
    segment_of_dir(parts.next()?)
}

impl StillSet {
    /// `(顯示名稱, 路徑, 圖)`，依檔名排序後處理（樣板編號、每格相似度的順序跟著走，順序要固定）。
    /// 用檔名不用完整路徑排：靜圖從一段的子資料夾搬到另一段，順序不變、不用重新掃描
    pub fn build(mut images: Vec<(String, String, RgbImage)>) -> StillSet {
        images.sort_by(|a, b| file_name(&a.1).cmp(file_name(&b.1)).then_with(|| a.1.cmp(&b.1)));
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
                        let generic = m.text_lines() <= 1;
                        templates.push(Template { layout, mask: m, stills: vec![idx], generic });
                        Some(templates.len() - 1)
                    }
                }
            } else {
                None
            };
            let s320 = imageops::resize(&img, 320, 180, imageops::FilterType::Triangle);
            let small = small_gray(&s320, layout);
            let segment = segment_of_path(&path);
            stills.push(Still { name, path, segment, layout, template, small });
        }
        StillSet { stills, templates, off: Vec::new() }
    }

    /// 第 n 段能用的靜圖：共用的（直接放在資料夾裡）＋放在這段子資料夾的。
    /// 靜圖編號不變（每格的相似度照舊對得上），別段的標成 off、從來源字樣板底下拿掉。沒有要排除的回傳 None
    pub fn for_segment(&self, n: usize) -> Option<StillSet> {
        if !self.off.is_empty() {
            return None; // 已經是某一段的
        }
        let off: Vec<bool> = self.stills.iter().map(|s| s.segment.is_some_and(|g| g != n)).collect();
        if !off.contains(&true) {
            return None;
        }
        let templates = self.templates.iter().map(|t| Template { stills: t.stills.iter().copied().filter(|&k| !off[k]).collect(), ..t.clone() }).collect();
        Some(StillSet { stills: self.stills.clone(), templates, off })
    }

    pub fn is_off(&self, k: usize) -> bool {
        self.off.get(k).copied().unwrap_or(false)
    }

    /// 快取簽名：每張靜圖的檔名＋版型＋樣板編號。靜圖增減、換圖造成判斷改變，簽名就不同
    pub fn signature(&self) -> String {
        self.stills
            .iter()
            .map(|s| format!("{}|{}|{}", file_name(&s.path), s.layout == Layout::Dual, s.template.map(|t| t as i64).unwrap_or(-1)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 報告開頭的靜圖清單
    pub fn report(&self) -> String {
        let mut r = format!("== 靜圖 {} 張、來源字樣板 {} 個 ==\n", self.stills.len(), self.templates.len());
        for s in &self.stills {
            let t = s.template.map(|k| format!("樣板 #{k}")).unwrap_or_else(|| "無來源字→比畫面".into());
            let seg = s.segment.map(|n| format!("只用在第 {n} 段")).unwrap_or_else(|| "每段都比".into());
            r.push_str(&format!("  {}  {}  {}  {}\n", s.name, s.layout.zh(), t, seg));
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
        assert!(set.for_segment(1).is_none(), "沒有放進段落子資料夾的，不用排除");
    }

    #[test]
    fn stills_in_segment_folders_only_serve_that_segment() {
        assert_eq!(segment_of_path(r"\\10.3.20.33\yt上傳\替換畫面\第2段\a.jpg"), Some(2));
        assert_eq!(segment_of_path("/s/第六段/a.jpg"), Some(6));
        assert_eq!(segment_of_path("/s/a.jpg"), None);
        let a = still_with_label(1);
        let set = StillSet::build(vec![
            ("第3段/b.jpg".into(), "/s/第3段/b.jpg".into(), a.clone()),
            ("第2段/a.jpg".into(), "/s/第2段/a.jpg".into(), a),
            ("c.jpg".into(), "/s/c.jpg".into(), still_with_label(2)),
        ]);
        let names: Vec<&str> = set.stills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["第2段/a.jpg", "第3段/b.jpg", "c.jpg"], "依檔名排序，不看在哪個子資料夾");
        assert_eq!(set.templates[0].stills, vec![0, 1], "同一個來源字還是同一個樣板");
        let two = set.for_segment(2).unwrap();
        assert_eq!(two.templates[0].stills, vec![0], "第 2 段只看得到自己的和共用的");
        assert_eq!(two.off, vec![false, true, false]);
        let five = set.for_segment(5).unwrap();
        assert!(five.templates[0].stills.is_empty());
        assert_eq!(five.off, vec![true, true, false]);
        // 搬到別段：順序、簽名都不變（不用重新掃描）
        let moved = StillSet::build(vec![
            ("第3段/b.jpg".into(), "/s/第3段/b.jpg".into(), still_with_label(1)),
            ("第3段/a.jpg".into(), "/s/第3段/a.jpg".into(), still_with_label(1)),
            ("c.jpg".into(), "/s/c.jpg".into(), still_with_label(2)),
        ]);
        assert_eq!(moved.signature(), set.signature());
    }
}

/// 開發用：`KWT_STILLS=資料夾 cargo test -p keyword-title-core probe_stills -- --ignored --nocapture`
/// 列出每張靜圖的版型、樣板、來源字亮點數與行數
#[cfg(test)]
mod probe {
    use super::*;

    #[test]
    #[ignore]
    fn probe_stills() {
        let dir = std::env::var("KWT_STILLS").expect("KWT_STILLS");
        let mut images = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("jpg") || x.eq_ignore_ascii_case("png")) {
                let img = image::open(&p).unwrap().to_rgb8();
                images.push((p.file_name().unwrap().to_string_lossy().into_owned(), p.display().to_string(), img));
            }
        }
        let set = StillSet::build(images);
        for (k, t) in set.templates.iter().enumerate() {
            println!("template #{k} {:?} count={} lines={} generic={} stills={:?}", t.layout, t.mask.count, t.mask.text_lines(), t.generic, t.stills.iter().map(|&s| set.stills[s].name.clone()).collect::<Vec<_>>());
        }
    }
}
