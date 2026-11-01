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
    /// 輪播時確認來源相同用（`rotation_pool`）
    pub source: SourceText,
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
    /// 兩行以上：第二行從樣板的第幾列開始（比對時第二行也要像，見 `Mask::iou_parts`）
    pub split: Option<usize>,
}

impl Template {
    /// 這一格的來源字跟樣板像不像：兩行以上取「整個」和「第二行以下」比較低的那個
    pub fn score(&self, label: &Mask) -> f32 {
        match self.split {
            Some(split) => {
                let (full, below) = self.mask.iou_parts(label, split);
                full.min(below)
            }
            None => self.mask.iou(label),
        }
    }
}

#[derive(Clone, Default)]
pub struct StillSet {
    pub stills: Vec<Still>,
    pub templates: Vec<Template>,
    /// `for_segment` 排除掉的靜圖（別段的）；空的＝全部可用
    pub off: Vec<bool>,
    /// 樣板 a、b 是不是同一個來源字（同一個樣板，或字形 IoU ≥ 0.5：同一個來源字的靜圖背景不同，沒併成一個樣板）
    pub kin: Vec<Vec<bool>>,
}

/// 來源字比對算法的版本（label-v3：兩行來源字第二行逐字比）
const SIGNATURE_VERSION: &str = "label-v3";

/// 兩張靜圖的來源字每一塊至少要重疊幾成才算同一個來源（見 `SourceText::same`）
const SAME_SOURCE: f32 = 0.65;

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
                        let lines = m.line_spans();
                        let generic = lines.len() <= 1;
                        // 第二行開頭往上留 2 列（對齊有 ±2 px 的誤差）
                        let split = lines.get(1).map(|l| l.0.saturating_sub(2));
                        templates.push(Template { layout, mask: m, stills: vec![idx], generic, split });
                        Some(templates.len() - 1)
                    }
                }
            } else {
                None
            };
            let s320 = imageops::resize(&img, 320, 180, imageops::FilterType::Triangle);
            let small = small_gray(&s320, layout);
            let segment = segment_of_path(&path);
            let source = SourceText::from_image(&img, layout);
            stills.push(Still { name, path, segment, layout, template, source, small });
        }
        let kin = (0..templates.len())
            .map(|a| (0..templates.len()).map(|b| a == b || (templates[a].layout == templates[b].layout && templates[a].mask.iou(&templates[b].mask) >= 0.5)).collect())
            .collect();
        StillSet { stills, templates, off: Vec::new(), kin }
    }

    /// 樣板 a、b 是不是同一個來源字
    pub fn same_label(&self, a: usize, b: usize) -> bool {
        a == b || self.kin.get(a).and_then(|r| r.get(b)).copied().unwrap_or(false)
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
        Some(StillSet { stills: self.stills.clone(), templates, off, kin: self.kin.clone() })
    }

    pub fn is_off(&self, k: usize) -> bool {
        self.off.get(k).copied().unwrap_or(false)
    }

    /// 遮罩輪播可以用的靜圖：第一張是 `k`，後面接來源字一字不差的（同版型、這段能用）。
    /// 樣板分不出來：同一個來源字背景不同時會分到不同樣板（10/30 第 6 段 8 張都是「翻攝 X／亞美尼亞國會變格鬥場」，分在 4 個樣板）。
    /// 只有一行（「翻攝 X」）的不同素材常常一模一樣，沒有來源字的看不出來源，都不輪播
    pub fn rotation_pool(&self, k: usize) -> Vec<usize> {
        let s = &self.stills[k];
        let with_detail = |o: &Still| o.template.is_some_and(|t| !self.templates[t].generic);
        if !with_detail(s) {
            return vec![k];
        }
        let mut pool: Vec<usize> = (0..self.stills.len())
            .filter(|&o| o == k || (!self.is_off(o) && self.stills[o].layout == s.layout && with_detail(&self.stills[o]) && s.source.same(&self.stills[o].source)))
            .collect();
        let at = pool.iter().position(|&o| o == k).unwrap_or(0);
        pool.rotate_left(at);
        pool
    }

    /// 快取簽名：每張靜圖的檔名＋版型＋樣板編號。靜圖增減、換圖造成判斷改變，簽名就不同
    /// 開頭是比對算法的版本：每格來源字比對的算法改了就換，舊的掃描快取全部作廢重掃
    pub fn signature(&self) -> String {
        let stills = self.stills.iter().map(|s| format!("{}|{}|{}", file_name(&s.path), s.layout == Layout::Dual, s.template.map(|t| t as i64).unwrap_or(-1)));
        std::iter::once(SIGNATURE_VERSION.to_string()).chain(stills).collect::<Vec<_>>().join("\n")
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

/// 靜圖右上角來源字的點陣（比 `Mask` 範圍寬、只留白字黑邊的點），輪播時確認兩張靜圖的來源字一字不差
#[derive(Clone)]
pub struct SourceText {
    w: usize,
    h: usize,
    bits: Vec<bool>,
    /// 一塊約一個字寬（比對時逐塊比）
    block: (usize, usize),
}

impl SourceText {
    fn from_image(img: &RgbImage, layout: Layout) -> SourceText {
        // 滿版的字約 26×30；雙框的字小一號
        let ((x0, y0, w, h), block) = if layout == Layout::Full { ((1390, 85, 530, 120), (26, 30)) } else { ((700, 100, 340, 100), (16, 18)) };
        let mut bits = Vec::with_capacity((w * h) as usize);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let v = luma(&img.get_pixel(x, y).0);
                // 字是白的、外面有一圈黑邊
                let on = v >= 175.0 && (y.saturating_sub(3)..=(y + 3).min(H - 1)).any(|yy| (x.saturating_sub(3)..=(x + 3).min(W - 1)).any(|xx| luma(&img.get_pixel(xx, yy).0) <= v - 60.0));
                bits.push(on);
            }
        }
        SourceText { w: w as usize, h: h as usize, bits, block }
    }

    /// 兩張的字是不是一樣：整體對齊（±2 px）後逐塊比，取最差的一塊。
    /// 兩邊都有筆畫的塊才比（背景雜的圖會多出一些亮點，字不會少）；比得到的塊太少＝看不出來，不算一樣。
    /// 10/30 的靜圖：同來源最低 0.69，不同來源最高 0.52（「10 元」跟「1000 元」0.28）
    fn same(&self, o: &SourceText) -> bool {
        let at = |t: &SourceText, x: i32, y: i32| x >= 0 && y >= 0 && (x as usize) < t.w && (y as usize) < t.h && t.bits[y as usize * t.w + x as usize];
        let (w, h) = (self.w as i32, self.h as i32);
        let (mut best, mut off) = (-1i32, (0, 0));
        for dy in -2..=2 {
            for dx in -2..=2 {
                let mut inter = 0;
                for y in 0..h {
                    for x in 0..w {
                        inter += (at(self, x, y) && at(o, x + dx, y + dy)) as i32;
                    }
                }
                if inter > best {
                    (best, off) = (inter, (dx, dy));
                }
            }
        }
        let (bw, bh) = (self.block.0 as i32, self.block.1 as i32);
        let min_px = bw * bh / 32;
        let (mut compared, mut worst) = (0, 1f32);
        for y0 in (0..h).step_by(self.block.1) {
            for x0 in (0..w).step_by(self.block.0) {
                let (mut s, mut t, mut inter) = (0, 0, 0);
                for y in y0..(y0 + bh).min(h) {
                    for x in x0..(x0 + bw).min(w) {
                        let (p, q) = (at(self, x, y), at(o, x + off.0, y + off.1));
                        s += p as i32;
                        t += q as i32;
                        inter += (p && q) as i32;
                    }
                }
                if s >= min_px && t >= min_px {
                    compared += 1;
                    worst = worst.min(inter as f32 / s.min(t) as f32);
                }
            }
        }
        compared >= 4 && worst >= SAME_SOURCE
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
        assert!(set.signature().starts_with("label-v3\na.jpg|false|0\nb.jpg|false|0"));
        assert!(set.for_segment(1).is_none(), "沒有放進段落子資料夾的，不用排除");
    }

    #[test]
    fn rotation_only_uses_same_source_stills() {
        let a = still_with_label(1);
        let mut set = StillSet::build(vec![
            ("a.jpg".into(), "/s/a.jpg".into(), a.clone()),
            ("b.jpg".into(), "/s/b.jpg".into(), a.clone()),
            ("c.jpg".into(), "/s/c.jpg".into(), a),
            ("d.jpg".into(), "/s/d.jpg".into(), still_with_label(2)),
            ("e.jpg".into(), "/s/e.jpg".into(), RgbImage::from_pixel(W, H, image::Rgb([50, 50, 50]))),
        ]);
        // 測試圖的字形只算得出一行，這裡當成有說明的來源字
        set.templates.iter_mut().for_each(|t| t.generic = false);
        assert_eq!(set.rotation_pool(1), vec![1, 2, 0], "自己先，來源不同的不混進來");
        assert_eq!(set.rotation_pool(3), vec![3]);
        assert_eq!(set.rotation_pool(4), vec![4], "沒有來源字不輪播");
        let mut one_line = set.clone();
        one_line.templates.iter_mut().for_each(|t| t.generic = true);
        assert_eq!(one_line.rotation_pool(0), vec![0], "只有一行的來源字看不出是不是同一個來源");
        let m = crate::autocut::plan::MaskSeg { start: 0, end: (17.0 * crate::autocut::FPS) as usize, layout: Layout::Full, still: 0, by_label: true, iou: 0.9, ai: false };
        let fps = crate::autocut::FPS;
        let turns = crate::autocut::plan::rotate_mask(&set, &m, 7.0);
        assert_eq!(turns.iter().map(|t| t.still).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(turns[1].start, (7.0 * fps) as usize);
        assert_eq!(turns[2].end, m.end, "最後接到結尾");
        assert!(turns.windows(2).all(|w| w[0].end == w[1].start), "中間不能露出原畫面");
        let short = crate::autocut::plan::MaskSeg { end: (8.0 * fps) as usize, ..m.clone() };
        assert_eq!(crate::autocut::plan::rotate_mask(&set, &short, 7.0).len(), 1, "剩不到 2 秒不另外換一張");
        assert_eq!(crate::autocut::plan::rotate_mask(&set, &m, 0.0), vec![m.clone()], "0＝不輪播");
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
