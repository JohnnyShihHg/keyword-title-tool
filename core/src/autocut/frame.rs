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

    /// 來源字有幾行：有亮點（≥ 2 點）的列分成幾塊，中間空 3 列以上才算分開
    pub fn text_lines(&self) -> usize {
        let mut lines = 0;
        let mut blank = usize::MAX;
        for y in 0..self.h {
            let on = (0..self.w).filter(|&x| self.bits[y * self.w + x]).count() >= 2;
            if on {
                if blank >= 3 {
                    lines += 1;
                }
                blank = 0;
            } else {
                blank = blank.saturating_add(1);
            }
        }
        lines
    }

    /// 以外框左上角對齊（±2 px），回傳最大 IoU
    pub fn iou(&self, other: &Mask) -> f32 {
        if self.count == 0 || other.count == 0 {
            return 0.0;
        }
        let (lo, hi) = (self.count.min(other.count) as f32, self.count.max(other.count) as f32);
        if lo / hi < 0.3 {
            return lo / hi; // IoU 不可能超過這個比例，省得逐點比
        }
        let base_x = other.bbox.0 as i32 - self.bbox.0 as i32;
        let base_y = other.bbox.1 as i32 - self.bbox.1 as i32;
        let mut best = 0f32;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (ox, oy) = (base_x + dx, base_y + dy);
                let mut inter = 0usize;
                for y in 0..self.h as i32 {
                    let y2 = y + oy;
                    if y2 < 0 || y2 as usize >= other.h {
                        continue;
                    }
                    for x in 0..self.w as i32 {
                        let x2 = x + ox;
                        if x2 >= 0 && (x2 as usize) < other.w && self.bits[y as usize * self.w + x as usize] && other.bits[y2 as usize * other.w + x2 as usize] {
                            inter += 1;
                        }
                    }
                }
                best = best.max(inter as f32 / (self.count + other.count - inter) as f32);
            }
        }
        best
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
            let v = t.mask.iou(&label_mask);
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
    fn ncc_basics() {
        let a = GrayImage::from_fn(80, 45, |x, y| image::Luma([(x * 3 + y) as u8]));
        let inv = GrayImage::from_fn(80, 45, |x, y| image::Luma([255 - (x * 3 + y) as u8]));
        let flat = GrayImage::from_pixel(80, 45, image::Luma([100]));
        assert!((ncc(&a, &a) - 1.0).abs() < 1e-4);
        assert!((ncc(&a, &inv) + 1.0).abs() < 1e-4);
        assert_eq!(ncc(&a, &flat), 0.0);
    }
}
