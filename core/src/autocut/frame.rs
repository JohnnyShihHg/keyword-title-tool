//! 單格判斷：標題條、版型、來源字遮罩、相似度。輸入是 ffmpeg 拼好的分析畫布（見 `scan_filter`）。

use super::stills::StillSet;
use super::{FrameInfo, Layout, BAND, BAR, BOX, CW, LD, LF, ROI_DUAL, ROI_FULL, STRIP};
use image::{imageops, GrayImage, RgbImage};

// ───────────────────────── 來源字 ─────────────────────────

/// 二值化的來源字（比周圍背景亮一截的點）
#[derive(Clone, Debug)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub bits: Vec<bool>,
    pub bbox: (usize, usize),
    pub count: usize,
}

impl Mask {
    /// 亮點＝比 21×21 鄰近平均亮 30 以上、且本身 ≥ 120（白字、灰字都算，模糊背景不算）
    pub fn from_fn(w: usize, h: usize, f: impl Fn(usize, usize) -> f32) -> Mask {
        let lum: Vec<f32> = (0..w * h).map(|i| f(i % w, i / w)).collect();
        // 積分圖算鄰近平均
        let mut ii = vec![0f64; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = 0f64;
            for x in 0..w {
                row += lum[y * w + x] as f64;
                ii[(y + 1) * (w + 1) + x + 1] = ii[y * (w + 1) + x + 1] + row;
            }
        }
        let r = 10usize;
        let mut bits = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let (x0, y0, x1, y1) = (x.saturating_sub(r), y.saturating_sub(r), (x + r + 1).min(w), (y + r + 1).min(h));
                let s = ii[y1 * (w + 1) + x1] - ii[y0 * (w + 1) + x1] - ii[y1 * (w + 1) + x0] + ii[y0 * (w + 1) + x0];
                let mean = s / ((x1 - x0) * (y1 - y0)) as f64;
                let l = lum[y * w + x];
                bits[y * w + x] = l >= 120.0 && l as f64 - mean >= 30.0;
            }
        }
        let row = (0..h).find(|&y| (0..w).filter(|&x| bits[y * w + x]).count() >= 2);
        let col = (0..w).find(|&x| (0..h).filter(|&y| bits[y * w + x]).count() >= 2);
        let count = bits.iter().filter(|b| **b).count();
        Mask { w, h, bits, bbox: (col.unwrap_or(0), row.unwrap_or(0)), count }
    }

    /// 來源字有幾行。範圍最上面會切到「東森新聞」台標的字腳、亮的背景（天空、街景）也會被當成亮點，
    /// 所以要像一行字才算：有 ≥ 3 個亮點的列連成一塊（中間斷 3 列以內算同一塊），至少 6 列高、
    /// 平均每列 ≥ 8 點（雙框字小，≥ 4 點）；第二行以後要緊接在上一行下面（間隔 ≤ 15 列）。
    /// （10/28 第 3 段 lenskart 靜圖「翻攝 Youtube」以前被算成 3 行，單行來源字被當成證據，到處誤遮）
    pub fn text_lines(&self) -> usize {
        self.line_spans().len()
    }

    /// 每一行字的列範圍（起, 訖），規則見 `text_lines`
    pub fn line_spans(&self) -> Vec<(usize, usize)> {
        let counts: Vec<usize> = (0..self.h).map(|y| (0..self.w).filter(|&x| self.bits[y * self.w + x]).count()).collect();
        let min_mean = if self.w < ROI_FULL.2 as usize { 4.0 } else { 8.0 };
        // 一塊一塊：(起, 訖)
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for y in (0..self.h).filter(|&y| counts[y] >= 3) {
            match blocks.last_mut() {
                Some(b) if y - b.1 <= 4 => b.1 = y,
                _ => blocks.push((y, y)),
            }
        }
        let mut lines: Vec<(usize, usize)> = Vec::new();
        for (a, b) in blocks {
            let h = b - a + 1;
            let mean = counts[a..=b].iter().sum::<usize>() as f32 / h as f32;
            if h < 6 || mean < min_mean || lines.last().is_some_and(|l| a - l.1 > 15) {
                continue;
            }
            lines.push((a, b));
        }
        lines
    }

    /// 以外框左上角對齊（±2 px），回傳最大 IoU
    pub fn iou(&self, other: &Mask) -> f32 {
        self.iou_parts(other, self.h).0
    }

    /// 兩行以上的來源字：(整個的 IoU, 第二行以下逐字比最差的那個字的 IoU)，都在整個 IoU 最大的對齊位置算。
    /// `self` 是樣板，`split` 用樣板的座標（第二行開頭）。
    /// - 只看整個：「翻攝 抖音／藍澤鵬程」跟「翻攝 抖音／北京無人機新規」第一行一樣就 0.62（10/29 第 5 段 6:22 誤遮）
    /// - 第二行整行一起比：「中國觀音無人機」跟「中國仙鶴無人機」7 個字有 5 個一樣，還有 0.69；
    ///   同一支影片背景雜的時候也只有 0.70～0.76，分不開。切成一個字寬一塊逐塊比，不一樣的字那塊會掉很多
    pub fn iou_parts(&self, other: &Mask, split: usize) -> (f32, f32) {
        if self.count == 0 || other.count == 0 {
            return (0.0, 0.0);
        }
        let (lo, hi) = (self.count.min(other.count) as f32, self.count.max(other.count) as f32);
        if lo / hi < 0.3 {
            return (lo / hi, 0.0); // IoU 不可能超過這個比例，省得逐點比
        }
        let on_self = |x: i32, y: i32| self.bits[y as usize * self.w + x as usize];
        let on_other = |x: i32, y: i32| x >= 0 && y >= 0 && (x as usize) < other.w && (y as usize) < other.h && other.bits[y as usize * other.w + x as usize];
        let base_x = other.bbox.0 as i32 - self.bbox.0 as i32;
        let base_y = other.bbox.1 as i32 - self.bbox.1 as i32;
        let (mut full, mut at) = (0f32, (base_x, base_y));
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (ox, oy) = (base_x + dx, base_y + dy);
                let mut inter = 0usize;
                for y in 0..self.h as i32 {
                    for x in 0..self.w as i32 {
                        if on_self(x, y) && on_other(x + ox, y + oy) {
                            inter += 1;
                        }
                    }
                }
                let v = inter as f32 / (self.count + other.count - inter) as f32;
                if v > full {
                    (full, at) = (v, (ox, oy));
                }
            }
        }
        // 第二行以下：樣板有字的範圍切成一個字寬（≈ 行高）一塊
        let (ox, oy) = at;
        let split = split.min(self.h) as i32;
        let rows: Vec<i32> = (split..self.h as i32).filter(|&y| (0..self.w as i32).any(|x| on_self(x, y))).collect();
        let cols: Vec<i32> = (0..self.w as i32).filter(|&x| (split..self.h as i32).any(|y| on_self(x, y))).collect();
        let (Some(&y0), Some(&y1), Some(&x0), Some(&x1)) = (rows.first(), rows.last(), cols.first(), cols.last()) else { return (full, 0.0) };
        let char_w = ((y1 - y0 + 1) as f32 * 0.9).max(8.0) as i32;
        let n = (((x1 - x0 + 1) as f32 / char_w as f32).round() as i32).max(1);
        let mut worst = 1f32;
        for k in 0..n {
            let (a, b) = (x0 + (x1 - x0 + 1) * k / n, x0 + (x1 - x0 + 1) * (k + 1) / n);
            let (mut s, mut o, mut inter) = (0usize, 0usize, 0usize);
            for y in y0..=y1 {
                for x in a..b {
                    let (p, q) = (on_self(x, y), on_other(x + ox, y + oy));
                    s += p as usize;
                    o += q as usize;
                    inter += (p && q) as usize;
                }
            }
            if s < 15 {
                continue; // 筆畫太少（字縫、標點）
            }
            worst = worst.min(inter as f32 / (s + o - inter) as f32);
        }
        (full, worst)
    }
}

pub fn luma(p: &[u8]) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

/// 比相似度的區域（320×180 座標）：滿版取字幕以上，雙框取左框
pub fn sim_region(layout: Layout) -> (u32, u32, u32, u32) {
    match layout {
        Layout::Full => (0, 0, 320, BAND.0 / 6),
        Layout::Dual => (BOX.0 / 6, BOX.1 / 6, (BOX.2 - BOX.0) / 6, (BOX.3 - BOX.1) / 6),
    }
}

pub fn small_gray(img320: &RgbImage, layout: Layout) -> GrayImage {
    let (x, y, w, h) = sim_region(layout);
    let crop = imageops::crop_imm(img320, x, y, w, h).to_image();
    imageops::resize(&imageops::grayscale(&crop), 80, 45, imageops::FilterType::Triangle)
}

pub fn ncc(a: &GrayImage, b: &GrayImage) -> f32 {
    let n = a.len() as f32;
    let ma = a.iter().map(|&v| v as f32).sum::<f32>() / n;
    let mb = b.iter().map(|&v| v as f32).sum::<f32>() / n;
    let (mut s, mut sa, mut sb) = (0f32, 0f32, 0f32);
    for (&x, &y) in a.iter().zip(b.iter()) {
        let (dx, dy) = (x as f32 - ma, y as f32 - mb);
        s += dx * dy;
        sa += dx * dx;
        sb += dy * dy;
    }
    if sa < 1.0 || sb < 1.0 {
        0.0
    } else {
        s / (sa.sqrt() * sb.sqrt())
    }
}

pub type Strip = [[f32; 48]; 72];

/// 雙框兩框之間的縫每列都是「暗（944–947）、亮（951–954）、暗（973–976）」——這條縫是固定的框線，跟框裡播什麼無關。
/// （舊版還要求右框左緣 980–983 夠亮，右框剛好是暗的鏡頭就會誤判成滿版，已拿掉）
pub fn layout_from_strip(s: &Strip) -> Layout {
    let min = |row: &[f32; 48], a: usize, b: usize| row[a..=b].iter().cloned().fold(f32::MAX, f32::min);
    let mean = |row: &[f32; 48], a: usize, b: usize| row[a..=b].iter().sum::<f32>() / (b - a + 1) as f32;
    let ok = s
        .iter()
        .filter(|r| {
            let (d1, b1, d2) = (min(r, 6, 9), mean(r, 13, 16), min(r, 35, 38));
            d1 < 65.0 && b1 > 85.0 && b1 - d1 > 40.0 && d2 < 65.0 && b1 - d2 > 40.0
        })
        .count();
    if ok as f32 / 72.0 > 0.6 {
        Layout::Dual
    } else {
        Layout::Full
    }
}

/// 標題條上下兩條深藍邊線都在＝節目畫面
pub fn bar_present(canvas: &[u8]) -> bool {
    let row_ok = |r: usize| {
        let (mut rs, mut gs, mut bs) = (0f32, 0f32, 0f32);
        for x in 0..BAR.2 {
            let i = ((BAR.1 + r) * CW + BAR.0 + x) * 3;
            rs += canvas[i] as f32;
            gs += canvas[i + 1] as f32;
            bs += canvas[i + 2] as f32;
        }
        let k = BAR.2 as f32;
        rs / k < 45.0 && bs / k > 90.0 && bs / k > gs / k + 35.0
    };
    (4..=16).any(row_ok) && (92..=106).any(row_ok)
}

/// 判斷一格。回傳判斷結果和整張畫面的 80×45 灰階（算鏡頭切換用，`cut_diff` 由呼叫端跟前一格比）
pub fn judge(buf: &[u8], set: &StillSet) -> (FrameInfo, GrayImage) {
    let mut s320 = RgbImage::new(320, 180);
    for y in 0..180 {
        for x in 0..320 {
            let i = (y * CW + x) * 3;
            s320.put_pixel(x as u32, y as u32, image::Rgb([buf[i], buf[i + 1], buf[i + 2]]));
        }
    }
    let px = |cx: usize, cy: usize| luma(&buf[(cy * CW + cx) * 3..(cy * CW + cx) * 3 + 3]);
    let mut strip: Strip = [[0f32; 48]; 72];
    for (r, row) in strip.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = px(STRIP.0 + c, STRIP.1 + r);
        }
    }
    let layout = layout_from_strip(&strip);
    let label_mask = match layout {
        Layout::Full => Mask::from_fn(ROI_FULL.2 as usize, ROI_FULL.3 as usize, |x, y| px(LF.0 + x, LF.1 + y)),
        Layout::Dual => Mask::from_fn(ROI_DUAL.2 as usize, ROI_DUAL.3 as usize, |x, y| px(LD.0 + x, LD.1 + y)),
    };
    let (mut tmpl, mut iou) = (-1i16, 0f32);
    for (k, t) in set.templates.iter().enumerate() {
        if t.layout == layout {
            let v = t.score(&label_mask);
            if v > iou {
                iou = v;
                tmpl = k as i16;
            }
        }
    }
    let g = small_gray(&s320, layout);
    let sims = set.stills.iter().map(|s| if s.layout == layout { ncc(&s.small, &g) } else { 0.0 }).collect();
    let whole = imageops::resize(&imageops::grayscale(&s320), 80, 45, imageops::FilterType::Triangle);
    let lum = whole.iter().map(|&v| v as f32).sum::<f32>() / whole.len() as f32;
    (FrameInfo { program: bar_present(buf), layout, luma: lum, cut_diff: 0.0, tmpl, iou, sims }, whole)
}

/// 兩格整張畫面 80×45 灰階的平均差（鏡頭切換）
pub fn frame_diff(prev: &GrayImage, cur: &GrayImage) -> f32 {
    prev.iter().zip(cur.iter()).map(|(a, b)| (*a as f32 - *b as f32).abs()).sum::<f32>() / cur.len() as f32
}

/// Logo 轉場樣板 → 80×45 灰階（跟 `refine_start` 解出來的小圖比）
pub fn logo_template(img: &image::DynamicImage) -> GrayImage {
    imageops::resize(&imageops::grayscale(img), super::LOGO_W, super::LOGO_H, imageops::FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Vec<u8> {
        vec![0u8; super::super::CANVAS_BYTES]
    }

    fn paint_row(c: &mut [u8], r: usize, rgb: [u8; 3]) {
        for x in 0..BAR.2 {
            let i = ((BAR.1 + r) * CW + BAR.0 + x) * 3;
            c[i..i + 3].copy_from_slice(&rgb);
        }
    }

    #[test]
    fn text_lines_ignore_logo_tip_and_background_streak() {
        // 字：一行在 y 15..40；第二行（兩行來源字）在 y 50..75
        let label = |x: usize, y: usize| (20..160).contains(&x) && (15..40).contains(&y) && (x / 3 + y) % 4 == 0;
        let second = |x: usize, y: usize| (10..170).contains(&x) && (50..75).contains(&y) && (x / 3 + y) % 4 == 0;
        // 最上面一列台標字腳 4 點、最下面兩列背景亮邊
        let noise = |x: usize, y: usize| (y == 0 && (60..64).contains(&x)) || (y >= 88 && (30..56).contains(&x));
        let m = |f: &dyn Fn(usize, usize) -> bool| Mask::from_fn(180, 90, |x, y| if f(x, y) { 230.0 } else { 40.0 });
        assert_eq!(m(&|x, y| label(x, y)).text_lines(), 1);
        assert_eq!(m(&|x, y| label(x, y) || noise(x, y)).text_lines(), 1, "雜訊不算一行");
        assert_eq!(m(&|x, y| label(x, y) || second(x, y)).text_lines(), 2);
        assert_eq!(m(&|x, y| label(x, y) || second(x, y) || noise(x, y)).text_lines(), 2);
        assert_eq!(m(&|_, _| false).text_lines(), 0);
    }

    #[test]
    fn bar_needs_both_navy_lines() {
        let mut c = canvas();
        assert!(!bar_present(&c));
        paint_row(&mut c, 10, [10, 40, 140]);
        assert!(!bar_present(&c), "只有上線不算");
        paint_row(&mut c, 100, [10, 40, 140]);
        assert!(bar_present(&c));
    }

    #[test]
    fn strip_dark_bright_dark_is_dual() {
        let mut s: Strip = [[120f32; 48]; 72];
        assert_eq!(layout_from_strip(&s), Layout::Full);
        for row in s.iter_mut() {
            row[6..=9].fill(20.0);
            row[13..=16].fill(150.0);
            row[35..=38].fill(20.0);
            // 右框剛好是暗的也照樣算雙框（舊版規則的誤判）
            row[42..=45].fill(10.0);
        }
        assert_eq!(layout_from_strip(&s), Layout::Dual);
        // 超過四成的列不符合 → 滿版
        for row in s.iter_mut().take(30) {
            row[13..=16].fill(40.0);
        }
        assert_eq!(layout_from_strip(&s), Layout::Full);
    }

    #[test]
    fn mask_iou_identical_and_shifted() {
        let glyph = |ox: usize| move |x: usize, y: usize| if (20 + ox..40 + ox).contains(&x) && (30..34).contains(&y) || (25 + ox..28 + ox).contains(&x) && (20..50).contains(&y) { 230.0 } else { 40.0 };
        let a = Mask::from_fn(100, 70, glyph(0));
        let b = Mask::from_fn(100, 70, glyph(7));
        assert!(a.count > 50);
        assert!((a.iou(&a) - 1.0).abs() < 1e-6);
        assert!((a.iou(&b) - 1.0).abs() < 1e-6, "外框對齊後位移不影響");
        let empty = Mask::from_fn(100, 70, |_, _| 40.0);
        assert_eq!(a.iou(&empty), 0.0);
    }

    #[test]
    fn text_lines_ignore_bright_background() {
        // 滿版來源字區 180×90：每列亮點數照 10/28 實測的樣子做
        let mask = |rows: &[(usize, usize, usize)]| {
            let mut bits = vec![false; 180 * 90];
            for &(a, b, n) in rows {
                for y in a..=b {
                    bits[y * 180..y * 180 + n].fill(true);
                }
            }
            Mask { w: 180, h: 90, count: bits.iter().filter(|b| **b).count(), bits, bbox: (0, 0) }
        };
        // 「翻攝 Youtube」＋下面零星亮點、離很遠的一塊亮背景、最底下兩列
        assert_eq!(mask(&[(15, 39, 50), (50, 50, 4), (59, 60, 10), (73, 82, 25), (88, 89, 40)]).text_lines(), 1);
        // 兩行（「翻攝 X」＋「藍澤鵬程」）
        assert_eq!(mask(&[(15, 40, 50), (50, 74, 60)]).text_lines(), 2);
        // 雙框（110 寬）第一行很淡也算
        let mut dual = Mask { w: 110, h: 75, bits: vec![false; 110 * 75], bbox: (0, 0), count: 0 };
        for y in 17..=31 {
            dual.bits[y * 110..y * 110 + if y % 2 == 0 { 3 } else { 6 }].fill(true);
        }
        for y in 41..=55 {
            dual.bits[y * 110..y * 110 + 20].fill(true);
        }
        assert_eq!(dual.text_lines(), 2);
    }

    #[test]
    fn two_line_labels_compare_the_second_line_char_by_char() {
        // 「翻攝 抖音／中國仙鶴無人機」對「翻攝 抖音／中國觀音無人機」：第一行一樣、第二行 7 個字差 2 個
        let glyph = |x: usize, y: usize, k: usize, alt: bool| {
            // 每個字 22×22 格子裡畫一個跟 k 有關的筆畫圖案；alt＝換成別的字
            let (cx, cy) = (x % 24, y % 30);
            let s = if alt { k * 7 + 3 } else { k * 3 };
            cx < 22 && cy < 22 && ((cx + cy * s) % 5 == 0 || (cx * (s + 1) + cy) % 7 == 0)
        };
        let label = |alt: &[usize]| {
            Mask::from_fn(180, 90, |x, y| {
                let line1 = (14..36).contains(&y) && (20..20 + 24 * 5).contains(&x) && glyph(x - 20, y - 14, (x - 20) / 24, false);
                let line2 = (50..72).contains(&y) && (4..4 + 24 * 7).contains(&x) && {
                    let k = (x - 4) / 24;
                    glyph(x - 4, y - 50, k + 10, alt.contains(&k))
                };
                if line1 || line2 { 230.0 } else { 40.0 }
            })
        };
        let tmpl = label(&[]);
        let spans = tmpl.line_spans();
        assert_eq!(spans.len(), 2, "{spans:?}");
        let split = spans[1].0 - 2;
        let (full, worst) = tmpl.iou_parts(&label(&[]), split);
        assert!(full > 0.95 && worst > 0.95, "同一個來源字：{full} {worst}");
        let (full, worst) = tmpl.iou_parts(&label(&[2, 3]), split);
        assert!(full > 0.6, "整個比還是很像：{full}");
        assert!(worst < 0.5, "逐字比，不一樣的字那塊掉很多：{worst}");
    }

    #[test]
    fn ncc_basics() {
        let a = GrayImage::from_fn(80, 45, |x, y| image::Luma([(x * 3 + y) as u8]));
        let inv = GrayImage::from_fn(80, 45, |x, y| image::Luma([255 - (x * 3 + y) as u8]));
        let flat = GrayImage::from_pixel(80, 45, image::Luma([100]));
        assert!((ncc(&a, &a) - 1.0).abs() < 1e-4);
        assert!((ncc(&a, &inv) + 1.0).abs() < 1e-4);
        assert_eq!(ncc(&a, &flat), 0.0);
    }
}
