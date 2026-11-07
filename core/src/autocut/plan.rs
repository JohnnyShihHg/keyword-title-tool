//! 規劃：節目區塊、切點、完整性、分段、遮罩，以及報告的文字。

use super::frame::ncc;
use super::stills::StillSet;
use super::titles::{which_segment, Titles};
use super::{secs, tc, FrameInfo, Layout, CUT_DIFF, END_TRIM, FLASH_LUMA, FPS, LABEL_IOU, LABEL_IOU_STRONG, SIM_NCC};
use image::GrayImage;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// 布林序列 → 區段：間隔 ≤ fill_gap 的補起來，短於 min_len 的丟掉
pub fn runs(flags: &[bool], fill_gap: usize, min_len: usize) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        if !flags[i] {
            i += 1;
            continue;
        }
        let s = i;
        while i < flags.len() && flags[i] {
            i += 1;
        }
        match v.last_mut() {
            Some(last) if s - last.1 <= fill_gap => last.1 = i,
            _ => v.push((s, i)),
        }
    }
    v.retain(|(s, e)| e - s >= min_len);
    v
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub n: usize,
    /// 側錄時間軸上的片段（格），通常只有一段；區塊內換段時會接起來
    pub pieces: Vec<(usize, usize)>,
    /// 開頭前看得到轉場／廣告（或是 OCR 換段點）
    pub start_ok: bool,
    /// 結尾後看得到轉場／廣告（或是 OCR 換段點）
    pub end_ok: bool,
}

impl Segment {
    pub fn complete(&self) -> bool {
        self.start_ok && self.end_ok
    }

    pub fn start(&self) -> usize {
        self.pieces[0].0
    }

    pub fn end(&self) -> usize {
        self.pieces.last().unwrap().1
    }

    pub fn duration(&self) -> f64 {
        self.pieces.iter().map(|(s, e)| secs(e - s)).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaskSeg {
    pub start: usize,
    pub end: usize,
    pub layout: Layout,
    pub still: usize,
    pub by_label: bool,
    pub iou: f32,
    /// 規則沒把握、交給 AI 判斷後補上的
    #[serde(default)]
    pub ai: bool,
    /// 報告裡額外說明這段是怎麼補上的（夾心空檔、AI 接上相鄰鏡頭）
    #[serde(skip)]
    pub note: Option<&'static str>,
}

pub fn is_cut(frames: &[FrameInfo], i: usize) -> bool {
    frames[i].cut_diff > CUT_DIFF
}

/// 整個鏡頭遮的時候，最多從命中的格往前後各延伸幾秒（鏡頭切換漏判時不會一路遮下去）
pub const SHOT_EXTEND_SECS: f64 = 15.0;
/// 整個鏡頭遮的時候，「還看得到同一個來源字」的門檻（比 LABEL_IOU 寬：只是確認還在同一支影片，不是拿來單獨判斷）
pub const SHOT_LABEL_IOU: f32 = 0.4;

/// 換鏡頭：畫面差很多，而且不是閃光（亮度突然大變、1 秒內又回到原本的亮度，例如爆炸、閃光燈）。
/// 閃光的開頭（之後亮度回來）和結尾（回到閃光前的亮度）都不算
pub fn is_shot_cut(frames: &[FrameInfo], i: usize) -> bool {
    if i == 0 || !is_cut(frames, i) {
        return false;
    }
    let (before, now) = (frames[i - 1].luma, frames[i].luma);
    if (now - before).abs() > FLASH_LUMA {
        let fps = FPS as usize;
        let starts = frames[i + 1..(i + fps).min(frames.len())].iter().any(|f| (f.luma - before).abs() < FLASH_LUMA / 2.0);
        let ends = frames[i.saturating_sub(fps)..i - 1].iter().any(|f| (f.luma - now).abs() < FLASH_LUMA / 2.0);
        return !starts && !ends;
    }
    true
}

/// range 裡的鏡頭（起, 訖）
pub fn shots(frames: &[FrameInfo], (r0, r1): (usize, usize)) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut a = r0;
    for i in r0 + 1..r1 {
        if is_shot_cut(frames, i) {
            out.push((a, i));
            a = i;
        }
    }
    if a < r1 {
        out.push((a, r1));
    }
    out
}

/// 節目區塊 → 依 OCR 分成第幾段。`ocr`：側錄時間軸上的格 → 標題條 OCR 文字。
/// 段落結尾已經扣掉 END_TRIM；開頭還沒對到 Logo 轉場（見 `refine_window`／`logo_end`）。
pub fn plan_segments(frames: &[FrameInfo], ocr: &BTreeMap<usize, String>, titles: &Titles, log: &mut String) -> Vec<Segment> {
    let n = frames.len();
    let fps = FPS as usize;
    let blocks = runs(&frames.iter().map(|f| f.program).collect::<Vec<_>>(), 2 * fps, fps);
    // 開頭往前找 Logo 轉場切進棚內的那一格（3 秒內）；結尾停在標題條消失處（就是切進廣告轉場的地方）
    let blocks: Vec<(usize, usize)> = blocks
        .into_iter()
        .map(|(s, e)| {
            let s2 = (s.saturating_sub(3 * fps)..=s).rev().find(|&i| i > 0 && is_cut(frames, i)).unwrap_or(s);
            let e2 = (e..(e + fps / 2).min(n)).find(|&i| is_cut(frames, i)).unwrap_or(e);
            (s2, e2)
        })
        .collect();
    // OCR：每個取樣格 → 段落
    let labels: Vec<(usize, usize)> = ocr.iter().filter_map(|(f, text)| which_segment(text, titles).map(|(seg, _)| (*f, seg))).collect();
    log.push_str("\n== 節目區塊（標題條）＋ OCR 段落 ==\n");
    let mut pieces: Vec<(usize, usize, usize)> = Vec::new(); // (段, 起, 訖)
    for &(s, e) in &blocks {
        let inside: Vec<(usize, usize)> = labels.iter().filter(|(f, _)| *f >= s && *f < e).cloned().collect();
        let mut count: BTreeMap<usize, usize> = BTreeMap::new();
        for (_, g) in &inside {
            *count.entry(*g).or_default() += 1;
        }
        log.push_str(&format!("  {} – {}（{:.0} 秒）OCR：{:?}\n", tc(secs(s)), tc(secs(e)), secs(e - s), count));
        // 雜訊：只出現 1～2 次的段落不算
        let valid: Vec<usize> = count.iter().filter(|(_, c)| **c >= 3).map(|(g, _)| *g).collect();
        if valid.is_empty() {
            log.push_str("    → 沒有對到文稿，不是本集節目，略過\n");
            continue;
        }
        // 區塊裡換段：找第一個「新段落連續出現 3 次」的位置，往前對齊到鏡頭切換點
        let mut cur_start = s;
        let mut cur = *valid.iter().min_by_key(|g| inside.iter().position(|(_, x)| x == *g).unwrap_or(usize::MAX)).unwrap();
        for k in 0..inside.len() {
            let (f, g) = inside[k];
            if g != cur && valid.contains(&g) && inside[k..].iter().take(3).all(|(_, x)| *x == g) {
                let prev_f = if k > 0 { inside[k - 1].0 } else { s };
                let split = (prev_f..f).rev().find(|&i| is_cut(frames, i)).unwrap_or(f);
                pieces.push((cur, cur_start, split));
                log.push_str(&format!("    → 第 {cur} 段到 {}，接著第 {g} 段\n", tc(secs(split))));
                cur = g;
                cur_start = split;
            }
        }
        pieces.push((cur, cur_start, e));
    }
    // 完整性：段落頭尾要嘛是 OCR 判斷的換段點，要嘛在素材裡看得到前後至少 5 秒的非節目畫面（轉場／廣告）。
    // 素材剛好斷在節目中間（就算那一刻標題條剛好消失）都算不完整。
    let splits: Vec<usize> = pieces.windows(2).filter(|w| w[0].2 == w[1].1).map(|w| w[0].2).collect();
    let nonprog = |a: usize, b: usize| frames[a..b].iter().filter(|f| !f.program).count() as f32 / (b - a).max(1) as f32;
    let edge_ok_start = |s: usize| splits.contains(&s) || (s >= 5 * fps && nonprog(s - 5 * fps, s) >= 0.8);
    let edge_ok_end = |e: usize| splits.contains(&e) || (e + 5 * fps <= n && nonprog(e, e + 5 * fps) >= 0.8);

    // 一段只取第一個區塊；之後又出現同一段的標題（節目後的預告、重播）不收。
    // 例外：文稿最後一段收尾常連進兩次廣告（10/27 第 6 段：播 2 分多 → 廣告 4 分半 → 又播 1 分多 → 收黑），
    // 中間沒有別段、隔不到 LAST_GAP_SECS 又出現的是同一段的下半部，直接接起來（不加轉場）
    let last = titles.keys().max().copied();
    let mut segs: BTreeMap<usize, Segment> = BTreeMap::new();
    for (k, (g, s, e)) in pieces.iter().copied().enumerate() {
        match segs.get_mut(&g) {
            None => {
                segs.insert(g, Segment { n: g, pieces: vec![(s, e)], start_ok: true, end_ok: true });
            }
            Some(seg) if seg.pieces.last().is_some_and(|p| p.1 == s) => seg.pieces.push((s, e)),
            Some(seg) if Some(g) == last && pieces[k - 1].0 == g && secs(s - seg.end()) <= LAST_GAP_SECS => {
                log.push_str(&format!("    → 第 {g} 段（最後一段）廣告後在 {} 繼續，接在後面\n", tc(secs(s))));
                seg.pieces.push((s, e));
            }
            Some(_) => log.push_str(&format!("    → 第 {g} 段在 {} 又出現（預告／重播），不收\n", tc(secs(s)))),
        }
    }
    for seg in segs.values_mut() {
        seg.start_ok = edge_ok_start(seg.start());
        seg.end_ok = if Some(seg.n) == last { last_end_ok(frames, seg.end()) } else { edge_ok_end(seg.end()) };
        // 結尾：人工習慣比切進廣告轉場早約 6 格收（中間隔著廣告的每一片都是）
        let ends: Vec<usize> = (0..seg.pieces.len()).filter(|&k| k + 1 == seg.pieces.len() || seg.pieces[k].1 != seg.pieces[k + 1].0).collect();
        for k in ends {
            let p = &mut seg.pieces[k];
            p.1 = p.1.saturating_sub(END_TRIM).max(p.0 + 1);
        }
    }
    segs.into_values().collect()
}

/// 最後一段廣告後還可能回來：隔多久內又出現算同一段；結尾後錄滿這麼久都沒回來也算結束
pub const LAST_GAP_SECS: f64 = 8.0 * 60.0;
/// 收黑：整張畫面平均亮度低於 BLACK_LUMA、連續 BLACK_SECS 秒（廣告之間偶爾閃一兩格黑不算）
pub const BLACK_LUMA: f32 = 24.0;
pub const BLACK_SECS: f64 = 5.0;

/// 最後一段的結尾：進廣告還不算完，要看到收黑；或結尾後已經錄滿 LAST_GAP_SECS 都沒回來（例如 2400 直接接新聞）
pub fn last_end_ok(frames: &[FrameInfo], e: usize) -> bool {
    if e + (LAST_GAP_SECS * FPS) as usize <= frames.len() {
        return true;
    }
    let need = (BLACK_SECS * FPS) as usize;
    let mut run = 0;
    for f in &frames[e.min(frames.len())..] {
        run = if f.luma < BLACK_LUMA { run + 1 } else { 0 };
        if run >= need {
            return true;
        }
    }
    false
}

/// `23:59:30`／`23:59` → 當天第幾秒；空白或看不懂回傳 None（不用這個條件）
pub fn parse_clock(s: &str) -> Option<u32> {
    let p: Vec<u32> = s.trim().replace('：', ":").split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    match p[..] {
        [h, m] if h < 24 && m < 60 => Some(h * 3600 + m * 60),
        [h, m, s] if h < 24 && m < 60 && s < 60 => Some(h * 3600 + m * 60 + s),
        _ => None,
    }
}

/// 最後一段靠時鐘判斷結束：側錄已經錄過 `cutoff`，標題條消失後也看得到 5 秒以上的非節目畫面。
/// 10/5～10/30 平日 18 天：節目都在 23:57:55～23:58:09 結束、最後一次廣告最晚 23:57:22 回來；
/// 但收尾有 5 天沒收黑（風景定格），當天資料夾又只錄到 00:00:40，收黑、錄滿 8 分鐘都等不到
/// （中間廣告的片頭底圖也會定格 4 分多，不能用定格判斷）。
/// `timeline_end` 是時間軸最後一格的時鐘（秒，過午夜會超過 86400）
pub fn clock_end_ok(frames: &[FrameInfo], e: usize, timeline_end: f64, cutoff: u32) -> bool {
    let need = 5 * FPS as usize;
    timeline_end >= cutoff as f64 && e + need <= frames.len() && frames[e..e + need].iter().filter(|f| !f.program).count() * 10 >= need * 8
}

/// 這一片的開頭要不要對 Logo 轉場：第一片、或前面隔著廣告的（區塊裡 OCR 換段接上的不用）
pub fn piece_after_break(seg: &Segment, k: usize) -> bool {
    k == 0 || seg.pieces[k - 1].1 + END_TRIM < seg.pieces[k].0
}

/// 段落開頭修準要解碼的範圍：標題條出現前 6 秒起、解 (s − s0 + 2 秒) 格。回傳 (起始格, 格數)
pub fn refine_window(s: usize) -> (usize, usize) {
    let win = (6.0 * FPS) as usize;
    let s0 = s.saturating_sub(win);
    (s0, s - s0 + 2 * FPS as usize)
}

/// 「關鍵時刻」Logo 轉場溶接到棚內結束的那一格（相對 `refine_window` 的起始格）：
/// 最後一格「還像 Logo」（≥0.6）之後、相似度掉到 <0.25 的第一格。之後呼叫端再 + START_DELAY。
pub fn logo_end(logo: &GrayImage, frames: &[GrayImage]) -> Option<usize> {
    let sims: Vec<f32> = frames.iter().map(|g| ncc(logo, g)).collect();
    let last_logo = sims.iter().rposition(|&v| v >= 0.6)?;
    (last_logo + 1..sims.len()).find(|&k| sims[k] < 0.25)
}

/// 來源字命中：IoU 夠高；或 IoU 普通但畫面也跟那個來源的靜圖有幾分像
/// （「翻攝 X ○○」不同主題外形很像，只看字形會把機器人當成忍者）。
/// 只有一行的來源字（「翻攝 X」）完全不當證據，只看畫面（SIM_NCC）：10/27 普吉島淹水的「翻攝 X」
/// 跟 iPhone 新聞的一模一樣，只看字形把淹水畫面整段蓋成 iPhone 的靜圖；
/// 「字形＋畫面有幾分像（0.6）」也還會在淹水畫面剛好有點像的一瞬間誤遮
pub fn label_hit(f: &FrameInfo, set: &StillSet) -> bool {
    if f.tmpl < 0 {
        return false;
    }
    let t = &set.templates[f.tmpl as usize];
    // 樣板底下的靜圖都是別段的（`StillSet::for_segment`）
    if t.generic || t.stills.is_empty() {
        return false;
    }
    // 兩行的來源字 f.iou 已經是逐字比最差的那個字（`Template::score`），只差兩個字的也會掉到 0.6 以下
    if f.iou >= LABEL_IOU_STRONG {
        return true;
    }
    f.iou >= LABEL_IOU && t.stills.iter().any(|&k| f.sims[k] >= 0.6)
}

/// 版型平滑：前後都是同一種版型、中間只維持不到 1.5 秒的另一種版型 → 當成前後那種（判斷偶爾抖一下不算數）
pub fn smooth_layouts(lay: &mut [Layout]) {
    let min_run = (1.5 * FPS) as usize;
    let mut k = 0;
    while k < lay.len() {
        let mut j = k;
        while j < lay.len() && lay[j] == lay[k] {
            j += 1;
        }
        if k > 0 && j < lay.len() && j - k < min_run && lay[k - 1] == lay[j] {
            let v = lay[j];
            lay[k..j].iter_mut().for_each(|x| *x = v);
        }
        k = j;
    }
}

pub fn plan_masks(frames: &[FrameInfo], set: &StillSet, range: (usize, usize)) -> Vec<MaskSeg> {
    let stills = &set.stills;
    let (r0, r1) = range;
    let fps = FPS as usize;
    let two_sec = 2 * fps;
    // 每格：要不要遮。閃光格（亮度突然大變）沿用前一格
    let mut hit = vec![false; r1 - r0];
    for i in r0..r1 {
        let f = &frames[i];
        let flash = i > 0 && (f.luma - frames[i - 1].luma).abs() > FLASH_LUMA;
        let h = label_hit(f, set) || f.sims.iter().any(|&s| s >= SIM_NCC);
        hit[i - r0] = if flash && i > r0 { hit[i - r0 - 1] } else { h };
    }
    let mut lay: Vec<Layout> = frames[r0..r1].iter().map(|f| f.layout).collect();
    smooth_layouts(&mut lay);
    let layout_at = |i: usize| lay[i - r0];
    // 滿版：有一格畫面跟靜圖幾乎一樣，整個鏡頭都遮（截圖只是那支影片的一兩格，
    // 10/28 攤販監視器爆炸那幾秒畫面變亮、人在動，跟截圖不像就露出來了）。雙框只有左框在換，看不出鏡頭切換，不套用。
    // 靜圖有來源字的：只延伸到還看得到同一個來源字的格（中間斷 1 秒內不算斷）。
    // 10/29 第 5 段 1:04–1:14 棚內電子板慢慢放大到仙鶴無人機的畫面，沒有硬切，整塊電子板被當成同一個鏡頭遮掉
    let lim = (SHOT_EXTEND_SECS * FPS) as usize;
    for (a, b) in shots(frames, (r0, r1)) {
        let matched: Vec<usize> = (a..b).filter(|&i| layout_at(i) == Layout::Full && frames[i].sims.iter().any(|&s| s >= SIM_NCC)).collect();
        let (Some(&m0), Some(&m1)) = (matched.first(), matched.last()) else { continue };
        let labels: BTreeSet<usize> = matched.iter().flat_map(|&i| (0..stills.len()).filter(move |&k| frames[i].sims[k] >= SIM_NCC)).filter_map(|k| stills[k].template).collect();
        let same_label = |i: usize| {
            let f = &frames[i];
            labels.is_empty() || (f.tmpl >= 0 && f.iou >= SHOT_LABEL_IOU && labels.iter().any(|&t| set.same_label(t, f.tmpl as usize)))
        };
        let (lo, hi) = (a.max(m0.saturating_sub(lim)), b.min(m1 + lim));
        // 從命中的格往外走，最後一個還有同一個來源字的格；連續 1 秒沒有就停
        let reach = |range: &mut dyn Iterator<Item = usize>| {
            let (mut last, mut miss) = (None, 0);
            for i in range {
                if same_label(i) {
                    last = Some(i);
                    miss = 0;
                } else {
                    miss += 1;
                    if miss > fps {
                        break;
                    }
                }
            }
            last
        };
        let start = reach(&mut (lo..m0).rev()).unwrap_or(m0);
        let end = reach(&mut (m1 + 1..hi)).map_or(m1 + 1, |i| i + 1);
        for i in start..end {
            if layout_at(i) == Layout::Full {
                hit[i - r0] = true;
            }
        }
    }
    let evidence = |f: &FrameInfo| label_hit(f, set) || f.sims.iter().any(|&s| s >= SIM_NCC);

    let mut segs: Vec<MaskSeg> = Vec::new();
    for (s, e) in runs(&hit, two_sec, 4) {
        let (mut s, mut e) = (s + r0, e + r0);
        // 靠畫面相似命中的靜圖：往前後延伸到「還算像」為止（雙框只有左框在換，整張畫面看不出鏡頭切換）
        let ks: Vec<usize> = (0..stills.len()).filter(|&k| frames[s..e].iter().any(|f| f.sims[k] >= SIM_NCC)).collect();
        if !ks.is_empty() {
            let like = |i: usize| ks.iter().any(|&k| frames[i].sims[k] >= 0.55);
            let lim = 10 * fps;
            while s > r0 && s + lim > e && like(s - 1) {
                s -= 1;
            }
            while e < r1 && e < s + lim + (e - s) && like(e) {
                e += 1;
            }
        }
        let start = (s.saturating_sub(5 * fps).max(r0)..=s).rev().find(|&c| is_cut(frames, c)).unwrap_or(s);
        let end = (e..(e + two_sec).min(r1)).find(|&c| is_cut(frames, c)).unwrap_or(e);
        let mut i = start;
        while i < end {
            let layout = layout_at(i);
            let mut j = i;
            while j < end && layout_at(j) == layout {
                j += 1;
            }
            // 這一小段本身沒有任何證據（只是補洞／延伸接進來的）→ 沿用前一段的版型和靜圖，不另外挑圖；
            // 前面沒有相連的段就不遮
            if !frames[i..j].iter().any(evidence) {
                if let Some(last) = segs.last_mut().filter(|l| l.end == i) {
                    last.end = j;
                }
                i = j;
                continue;
            }
            if let Some(last) = segs.last_mut() {
                if j - i < 10 && last.end == i {
                    last.end = j;
                    i = j;
                    continue;
                }
            }
            // 這段命中最多次的來源字樣板
            let mut votes: BTreeMap<i16, usize> = BTreeMap::new();
            let mut max_iou = 0f32;
            for f in &frames[i..j] {
                if label_hit(f, set) {
                    *votes.entry(f.tmpl).or_default() += 1;
                    max_iou = max_iou.max(f.iou);
                }
            }
            let mut score = vec![0f32; stills.len()];
            for f in &frames[i..j] {
                for (k, v) in f.sims.iter().enumerate() {
                    score[k] += v;
                }
            }
            let by_label = !votes.is_empty();
            // 靜圖候選：來源字命中 → 只用掛在那個樣板下的靜圖；否則 → 用畫面最像的
            let pool: Vec<usize> = if by_label {
                let t = *votes.iter().max_by_key(|(_, c)| **c).unwrap().0;
                set.templates[t as usize].stills.clone()
            } else {
                (0..stills.len()).filter(|&k| !set.is_off(k)).collect()
            };
            let same: Vec<usize> = pool.iter().cloned().filter(|&k| stills[k].layout == layout).collect();
            let pool = if same.is_empty() { pool } else { same };
            let still = *pool.iter().max_by(|a, b| score[**a].total_cmp(&score[**b])).unwrap();
            if let Some(last) = segs.last_mut() {
                if last.layout == layout && last.still == still && i >= last.end && i - last.end <= two_sec {
                    last.end = j;
                    i = j;
                    continue;
                }
            }
            segs.push(MaskSeg { start: i, end: j, layout, still, by_label, iou: max_iou, ai: false, note: None });
            i = j;
        }
    }
    // 雙框：左框通常是同一支素材連續播，來源字、相似度會斷斷續續。
    // 兩段雙框遮罩相隔 ≤ 8 秒、中間一直是雙框 → 併成一段，不讓中間露出原畫面。
    // 滿版：同一張靜圖（同一支影片）的兩段遮罩相隔 ≤ 8 秒、中間一直是滿版 → 也併起來（中間多半是同一支影片的其他鏡頭）
    let mut merged: Vec<MaskSeg> = Vec::new();
    for m in segs {
        if let Some(last) = merged.last_mut() {
            let near = m.layout == last.layout && m.start >= last.end && m.start - last.end <= 8 * fps && (last.end..m.start).all(|i| layout_at(i) == m.layout);
            if near && (m.layout == Layout::Dual || m.still == last.still) {
                last.end = m.end;
                continue;
            }
        }
        merged.push(m);
    }
    merged
}

/// 一段的所有遮罩（每個片段分開規劃）
pub fn segment_masks(frames: &[FrameInfo], set: &StillSet, seg: &Segment) -> Vec<MaskSeg> {
    if let Some((frames, set)) = for_segment(frames, set, seg) {
        return segment_masks(&frames, &set, seg);
    }
    seg.pieces.iter().flat_map(|&p| fill_sandwich_gaps(frames, &plan_masks(frames, set, p))).collect()
}

/// 夾心空檔最長幾秒（前後都在遮，中間這幾秒直接補）
pub const SANDWICH_SECS: f64 = 8.0;
pub const SANDWICH_NOTE: &str = "補上夾在中間的空檔";

/// 夾心空檔：兩段遮罩之間只隔 ≤ 8 秒、中間都是節目畫面 → 直接遮，不管前後是不是同一張靜圖、同一種版型、同一個來源字。
/// 11/02 第 2 段 3:52–3:56：前面滿版（畫面提供 TVBS.三立）、後面雙框，中間 4 秒辯論會近景來源字是一行的
/// 「翻攝 TVBS新聞 youtube」（一行的來源字不當證據），前後版型又不同、不會問 AI → 整段露出動態畫面。
/// 中間的畫面依版型分段：跟前一段同版型就接到前一段、跟後一段同版型就接到後一段，
/// 都不同就用同版型那一邊的靜圖另開一段；前後都沒有同版型的就不遮
pub fn fill_sandwich_gaps(frames: &[FrameInfo], masks: &[MaskSeg]) -> Vec<MaskSeg> {
    let lim = (SANDWICH_SECS * FPS) as usize;
    let mut out: Vec<MaskSeg> = Vec::new();
    for m in masks {
        let mut m = m.clone();
        if let Some(prev) = out.last().cloned() {
            let (g0, g1) = (prev.end, m.start);
            let program = |a: usize, b: usize| frames[a..b].iter().filter(|f| f.program).count() * 10 >= (b - a) * 9;
            if g1 > g0 && g1 - g0 <= lim && g1 <= frames.len() && program(g0, g1) {
                let mut lay: Vec<Layout> = frames[g0..g1].iter().map(|f| f.layout).collect();
                smooth_layouts(&mut lay);
                let mut i = g0;
                while i < g1 {
                    let l = lay[i - g0];
                    let mut j = i;
                    while j < g1 && lay[j - g0] == l {
                        j += 1;
                    }
                    let last = out.last_mut().unwrap();
                    if last.layout == l && last.end == i {
                        last.end = j;
                        last.note = Some(SANDWICH_NOTE);
                    } else if j == g1 && m.layout == l {
                        m.start = i;
                        m.note = Some(SANDWICH_NOTE);
                    } else if let Some(src) = [&prev, &m].into_iter().find(|s| s.layout == l).cloned() {
                        out.push(MaskSeg { start: i, end: j, ai: false, note: Some(SANDWICH_NOTE), ..src });
                    }
                    i = j;
                }
            }
        }
        out.push(m);
    }
    out
}

/// 只用這一段能用的靜圖（`StillSet::for_segment`）：段內每格跟別段靜圖的相似度改成 -1（當成完全不像）。
/// 沒有別段的靜圖要排除回傳 None
/// 別段的素材在這段重播的證據：畫面跟那張靜圖幾乎一樣，而且右上角是同一個來源字（沒有來源字、或只有一行的靜圖要畫面更像），
/// 連續 1 秒以上。11/06 第 4 段 17:58 重播第 1 段的 60 Minutes 訪談（跟第1段/Still059 0.8～0.97、來源字 0.85）、
/// 27:46 重播第 3 段的校園訪談，靜圖只放在第 1、3 段的子資料夾，第 4 段不比，整段露出來
pub const REUSE_SIM: f32 = 0.85;
pub const REUSE_SIM_NO_LABEL: f32 = 0.92;
pub const REUSE_SECS: f64 = 1.0;

/// 這段重播了哪幾段的素材：(那一段, 證據靜圖, 證據開始的格, 最高相似度)，每段只列第一個證據
pub fn reused_segments(frames: &[FrameInfo], set: &StillSet, seg: &Segment) -> Vec<(usize, usize, usize, f32)> {
    let need = (REUSE_SECS * FPS) as usize;
    let mut out: Vec<(usize, usize, usize, f32)> = Vec::new();
    for (k, s) in set.stills.iter().enumerate() {
        let Some(m) = s.segment.filter(|&m| m != seg.n) else { continue };
        if out.iter().any(|o| o.0 == m) {
            continue;
        }
        let label = s.template.filter(|&t| !set.templates[t].generic);
        let strong = |f: &FrameInfo| {
            let v = f.sims.get(k).copied().unwrap_or(0.0);
            match label {
                Some(t) => v >= REUSE_SIM && f.tmpl >= 0 && f.iou >= LABEL_IOU && set.same_label(t, f.tmpl as usize),
                None => v >= REUSE_SIM_NO_LABEL,
            }
        };
        'pieces: for &(a, b) in &seg.pieces {
            let (mut run, mut best) = (0usize, 0f32);
            for i in a..b.min(frames.len()) {
                if strong(&frames[i]) {
                    run += 1;
                    best = best.max(frames[i].sims[k]);
                    if run >= need {
                        out.push((m, k, i + 1 - run, best));
                        break 'pieces;
                    }
                } else {
                    run = 0;
                    best = 0.0;
                }
            }
        }
    }
    out.sort_by_key(|o| o.0);
    out
}

fn for_segment(frames: &[FrameInfo], set: &StillSet, seg: &Segment) -> Option<(Vec<FrameInfo>, StillSet)> {
    if (0..set.stills.len()).any(|k| set.is_off(k)) {
        return None; // 已經是某一段的
    }
    let also: Vec<usize> = reused_segments(frames, set, seg).iter().map(|o| o.0).collect();
    let set = set.for_segment_with(seg.n, &also)?;
    let (a, b) = (seg.start(), seg.end().min(frames.len()));
    let mut frames = frames.to_vec();
    for f in &mut frames[a..b] {
        for (k, v) in f.sims.iter_mut().enumerate() {
            if set.is_off(k) {
                *v = -1.0;
            }
        }
    }
    Some((frames, set))
}

/// 遮罩輪播：同來源的靜圖不只一張時，每 `every` 秒換下一張（輪完從頭），切成接在一起的幾段。
/// 只有一張、`every` ≤ 0 → 原樣。結尾剩不到 2 秒不另外換一張，併進前一張
pub fn rotate_mask(set: &StillSet, m: &MaskSeg, every: f64) -> Vec<MaskSeg> {
    let pool = set.rotation_pool(m.still);
    let step = (every * FPS) as usize;
    if pool.len() < 2 || step == 0 {
        return vec![m.clone()];
    }
    let mut out: Vec<MaskSeg> = Vec::new();
    let mut at = m.start;
    while at < m.end {
        let end = if m.end - at < step + (2.0 * FPS) as usize { m.end } else { at + step };
        out.push(MaskSeg { start: at, end, still: pool[out.len() % pool.len()], ..m.clone() });
        at = end;
    }
    out
}

/// 一段的遮罩套上輪播（只用這段能用的靜圖）
pub fn rotate_masks(set: &StillSet, n: usize, masks: &[MaskSeg], every: f64) -> Vec<MaskSeg> {
    let own = set.for_segment(n);
    let set = own.as_ref().unwrap_or(set);
    masks.iter().flat_map(|m| rotate_mask(set, m, every)).collect()
}

// ───────────────────────── 交給 AI 判斷的鏡頭 ─────────────────────────

/// 規則沒把握的一段畫面：取幾格右上角來源字給 AI 看，問「跟這些靜圖的來源字是不是同一個」
#[derive(Clone, Debug, PartialEq)]
pub struct AiCandidate {
    pub start: usize,
    pub end: usize,
    pub layout: Layout,
    /// 參考靜圖（同一個來源字的靜圖）
    pub stills: Vec<usize>,
    /// 給 AI 看的格（3 格，分散在這段裡）
    pub samples: Vec<usize>,
    pub kind: AiKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AiKind {
    /// 兩段同來源遮罩中間沒遮到的空檔（例：10/20 第 2 段 8:42–9:00，遠景鏡頭跟特寫靜圖不像、來源字太小比不到）
    Gap,
    /// 來源字比對分數在灰色地帶（0.15～0.62）、或畫面有幾分像（0.5～0.8）但沒達到門檻
    Weak,
    /// 滿版遮罩前後緊接著的鏡頭：問 AI「是不是同一場活動／同一支素材的畫面」（看整張畫面，不看來源字）。
    /// 11/02 第 2 段 8:25、8:34、10:52：辯論會同一場的其他鏡頭，來源字換成一行的「翻攝 TVBS新聞 youtube」、
    /// 或鏡頭跟靜圖不像，整個鏡頭露出來
    Scene,
}

/// 相鄰鏡頭最長看幾秒（沒找到鏡頭切換就只看這麼長）、最短幾秒才問
pub const SCENE_MAX_SECS: f64 = 15.0;
pub const SCENE_MIN_SECS: f64 = 1.0;
pub const SCENE_NOTE: &str = "AI 接上相鄰鏡頭（同一場畫面）";

/// 空檔最長幾秒還問 AI（太長多半中間換了別的內容）
pub const AI_GAP_MAX_SECS: f64 = 60.0;
/// 來源字灰色地帶的下限、畫面相似灰色地帶的下限（10/20 實測：0.5 會撈到大量其實是別的來源的鏡頭）
pub const AI_WEAK_IOU: f32 = 0.15;
pub const AI_WEAK_SIM: f32 = 0.65;
/// 灰色地帶至少要持續幾秒才問
pub const AI_WEAK_MIN_SECS: f64 = 2.0;
/// 灰色地帶每段最多問幾處（空檔一定問，不受限）
pub const AI_MAX_WEAK_PER_SEGMENT: usize = 12;

fn samples(s: usize, e: usize) -> Vec<usize> {
    [1, 2, 3].iter().map(|k| s + (e - s) * k / 4).collect()
}

/// 找出這一段裡要問 AI 的地方。`masks` 是 `segment_masks` 的結果（依時間排序）。
pub fn ai_candidates(frames: &[FrameInfo], set: &StillSet, seg: &Segment, masks: &[MaskSeg]) -> Vec<AiCandidate> {
    if let Some((frames, set)) = for_segment(frames, set, seg) {
        return ai_candidates(&frames, &set, seg, masks);
    }
    let fps = FPS as usize;
    let mut out = Vec::new();
    let template_of = |k: usize| set.stills[k].template;
    let in_piece = |a: usize, b: usize| seg.pieces.iter().any(|&(s, e)| a >= s && b <= e);
    // ① 兩段同來源遮罩之間的空檔
    for w in masks.windows(2) {
        let (m1, m2) = (&w[0], &w[1]);
        // 只有一行的來源字（「翻攝 X」）不算同一個來源的證據，要同一張靜圖
        let generic = |k: usize| template_of(k).is_some_and(|t| set.templates[t].generic);
        let same_source = m1.still == m2.still || (template_of(m1.still).is_some() && template_of(m1.still) == template_of(m2.still) && !generic(m1.still));
        let gap = m2.start.saturating_sub(m1.end);
        if !same_source || m1.layout != m2.layout || gap == 0 || gap as f64 > AI_GAP_MAX_SECS * FPS || !in_piece(m1.end, m2.start) {
            continue;
        }
        let g = &frames[m1.end..m2.start];
        let same_layout = g.iter().filter(|f| f.layout == m1.layout).count() * 10 >= g.len() * 9;
        let program = g.iter().filter(|f| f.program).count() * 10 >= g.len() * 9;
        if same_layout && program {
            let mut stills = vec![m1.still];
            if m2.still != m1.still {
                stills.push(m2.still);
            }
            out.push(AiCandidate { start: m1.end, end: m2.start, layout: m1.layout, stills, samples: samples(m1.end, m2.start), kind: AiKind::Gap });
        }
    }
    // ② 灰色地帶：沒被遮、但來源字或畫面有幾分像
    let gaps = out.len();
    let covered = |i: usize| masks.iter().any(|m| i >= m.start && i < m.end);
    for &(s, e) in &seg.pieces {
        let weak: Vec<bool> = (s..e)
            .map(|i| {
                let f = &frames[i];
                // AI 是看來源字判斷的：只有一行的來源字（「翻攝 X」）問了也一定說相同，不問
                let iou_weak = f.tmpl >= 0 && f.iou >= AI_WEAK_IOU && !set.templates[f.tmpl as usize].generic && !set.templates[f.tmpl as usize].stills.is_empty();
                let sim_weak = f.sims.iter().any(|&v| v >= AI_WEAK_SIM);
                f.program && !covered(i) && (iou_weak || sim_weak)
            })
            .collect();
        for (a, b) in runs(&weak, fps / 2, (AI_WEAK_MIN_SECS * FPS) as usize) {
            let (a, b) = (a + s, b + s);
            if out.iter().any(|c: &AiCandidate| a < c.end && b > c.start) {
                continue;
            }
            let layout = if frames[a..b].iter().filter(|f| f.layout == Layout::Dual).count() * 2 > b - a { Layout::Dual } else { Layout::Full };
            // 參考靜圖：最常出現的點陣樣板底下的靜圖＋畫面最像的那張（同版型）
            let mut votes: BTreeMap<i16, usize> = BTreeMap::new();
            let mut score = vec![0f32; set.stills.len()];
            for f in &frames[a..b] {
                if f.tmpl >= 0 && f.iou >= AI_WEAK_IOU && !set.templates[f.tmpl as usize].generic && !set.templates[f.tmpl as usize].stills.is_empty() {
                    *votes.entry(f.tmpl).or_default() += 1;
                }
                for (k, v) in f.sims.iter().enumerate() {
                    score[k] += v;
                }
            }
            let mut stills: Vec<usize> = votes.iter().max_by_key(|(_, c)| **c).map(|(t, _)| set.templates[*t as usize].stills.clone()).unwrap_or_default();
            if let Some(best) = (0..set.stills.len()).filter(|&k| set.stills[k].layout == layout && !set.is_off(k)).max_by(|x, y| score[*x].total_cmp(&score[*y])) {
                if score[best] / ((b - a) as f32) >= AI_WEAK_SIM && !stills.contains(&best) {
                    stills.push(best);
                }
            }
            stills.retain(|&k| set.stills[k].layout == layout || set.stills.iter().all(|s| s.layout != layout));
            stills.truncate(3);
            if !stills.is_empty() {
                out.push(AiCandidate { start: a, end: b, layout, stills, samples: samples(a, b), kind: AiKind::Weak });
            }
        }
    }
    // 空檔全部問；灰色地帶只問前幾處
    out.truncate(gaps + AI_MAX_WEAK_PER_SEGMENT);
    // ③ 滿版遮罩前後緊接著的鏡頭（雙框只有左框在換，看不出鏡頭切換，不問）
    let lim = (SCENE_MAX_SECS * FPS) as usize;
    let min_len = (SCENE_MIN_SECS * FPS) as usize;
    for (k, m) in masks.iter().enumerate() {
        if m.layout != Layout::Full {
            continue;
        }
        let prev_end = if k > 0 { masks[k - 1].end } else { 0 };
        let next_start = masks.get(k + 1).map_or(frames.len(), |n| n.start).min(frames.len());
        // 前面：往前找最近的鏡頭切換（最多 15 秒、不跨過前一段遮罩）
        let lo = m.start.saturating_sub(lim).max(prev_end);
        let before = ((lo + 1..m.start).rev().find(|&i| is_shot_cut(frames, i)).unwrap_or(lo), m.start);
        // 後面：到下一個鏡頭切換為止
        let hi = (m.end + lim).min(next_start);
        let after = (m.end, (m.end + 1..hi).find(|&i| is_shot_cut(frames, i)).unwrap_or(hi));
        for (a, b) in [before, after] {
            // 跟「灰色地帶」重疊也照樣問（灰色地帶問的是來源字、這裡問是不是同一場）：11/07 重跑 11/06 第 4 段 1:10:25，
            // 別段靜圖加進來後多了一個灰色地帶蓋住這個鏡頭，AI 說來源字不同，原本會接上的跳舞畫面就露出來。
            // 跟「空檔」重疊照樣問：11/02 第 1 段 12:33 遮罩後 51 秒的空檔大多是少康戰情室（AI 說來源不同），
            // 但緊接的前幾秒還是辯論會；14:51–15:00 腿部特寫讀不到來源字 → 空檔被判不同，整段露出
            if b <= a || b - a < min_len || !in_piece(a, b) || covered(a) || covered(b - 1) || out.iter().any(|c: &AiCandidate| c.kind == AiKind::Scene && a < c.end && b > c.start) {
                continue;
            }
            let g = &frames[a..b];
            let full = g.iter().filter(|f| f.layout == Layout::Full).count() * 10 >= g.len() * 9;
            let program = g.iter().filter(|f| f.program).count() * 10 >= g.len() * 9;
            if full && program {
                out.push(AiCandidate { start: a, end: b, layout: Layout::Full, stills: vec![m.still], samples: samples(a, b), kind: AiKind::Scene });
            }
        }
    }
    // 同一個起點：空檔先處理（空檔接起來後相鄰鏡頭就沒事了；反過來先接相鄰鏡頭會讓空檔對不到前後兩段）
    out.sort_by_key(|c| (c.start, c.kind == AiKind::Scene));
    out
}

// ───────────────────────── 手動遮罩 ─────────────────────────

pub const MANUAL_NOTE: &str = "手動指定";

/// 手動遮罩沒指定靜圖：這段時間裡平均畫面最像的那張（這段能用的靜圖；版型跟這段多數格一樣的優先）。
/// 11/06 第 1 段 21:04–21:13 歌劇的其他鏡頭：跟每張靜圖都只有 0.2～0.5，但最像的還是歌劇那幾張
pub fn pick_still(frames: &[FrameInfo], set: &StillSet, seg: &Segment, start: usize, end: usize) -> Option<usize> {
    let own = for_segment(frames, set, seg).map(|(_, s)| s);
    let set = own.as_ref().unwrap_or(set);
    let (a, b) = (start.min(frames.len()), end.min(frames.len()));
    let g = &frames[a..b];
    let dual = !g.is_empty() && g.iter().filter(|f| f.layout == Layout::Dual).count() * 2 > g.len();
    let layout = if dual { Layout::Dual } else { Layout::Full };
    let score = |k: usize| g.iter().map(|f| f.sims.get(k).copied().unwrap_or(0.0)).sum::<f32>();
    let usable: Vec<usize> = (0..set.stills.len()).filter(|&k| !set.is_off(k)).collect();
    let same: Vec<usize> = usable.iter().copied().filter(|&k| set.stills[k].layout == layout).collect();
    let pool = if same.is_empty() { usable } else { same };
    pool.into_iter().max_by(|x, y| score(*x).total_cmp(&score(*y)))
}

/// 手動指定的時段一定遮：跟原本的遮罩重疊的部分讓給手動的（原本的切掉重疊那截，前後剩下的照留）
pub fn apply_manual(masks: &mut Vec<MaskSeg>, start: usize, end: usize, still: usize, layout: Layout) {
    if end <= start {
        return;
    }
    let mut out = Vec::new();
    for m in masks.drain(..) {
        if m.end <= start || m.start >= end {
            out.push(m);
            continue;
        }
        if m.start < start {
            out.push(MaskSeg { end: start, ..m.clone() });
        }
        if m.end > end {
            out.push(MaskSeg { start: end, ..m.clone() });
        }
    }
    out.push(MaskSeg { start, end, layout, still, by_label: false, iou: 0.0, ai: false, note: Some(MANUAL_NOTE) });
    out.sort_by_key(|m| m.start);
    *masks = out;
}

/// AI 說「同一個來源」：空檔 → 把前後兩段遮罩接成一段；灰色地帶 → 補一段遮罩（用參考靜圖裡同版型、畫面最像的那張）
pub fn apply_ai(frames: &[FrameInfo], masks: &mut Vec<MaskSeg>, c: &AiCandidate) {
    match c.kind {
        AiKind::Gap => {
            if let Some(k) = masks.iter().position(|m| m.end == c.start) {
                if k + 1 < masks.len() && masks[k + 1].start == c.end {
                    let next = masks.remove(k + 1);
                    masks[k].end = next.end;
                    masks[k].ai = true;
                }
            }
        }
        AiKind::Weak => {
            let score = |k: usize| frames[c.start..c.end].iter().map(|f| f.sims.get(k).copied().unwrap_or(0.0)).sum::<f32>();
            let still = *c.stills.iter().max_by(|a, b| score(**a).total_cmp(&score(**b))).unwrap();
            masks.push(MaskSeg { start: c.start, end: c.end, layout: c.layout, still, by_label: true, iou: 0.0, ai: true, note: None });
            masks.sort_by_key(|m| m.start);
        }
        // 同一場畫面：接到出題的那段遮罩（前面的鏡頭往前接、後面的鏡頭往後接）
        AiKind::Scene => {
            let still = c.stills[0];
            // 這段已經被別的判斷（灰色地帶、空檔）遮了：不再疊一層
            if masks.iter().any(|m| m.start < c.end && m.end > c.start) {
                return;
            }
            if let Some(m) = masks.iter_mut().find(|m| m.end == c.start && m.still == still) {
                m.end = c.end;
                m.ai = true;
                m.note = Some(SCENE_NOTE);
            } else if let Some(m) = masks.iter_mut().find(|m| m.start == c.end && m.still == still) {
                m.start = c.start;
                m.ai = true;
                m.note = Some(SCENE_NOTE);
            }
        }
    }
}

// ───────────────────────── 報告 ─────────────────────────

/// 「== 段落 ==」那一節
pub fn report_segments(segs: &[(Segment, Vec<MaskSeg>)], set: &StillSet, rotate_secs: f64) -> String {
    let mut r = String::from("\n== 段落 ==\n");
    for (seg, masks) in segs {
        r.push_str(&format!("第 {} 段{}：長 {}，遮罩 {} 段\n", seg.n, if seg.complete() { "" } else { "（不完整，不輸出）" }, tc(seg.duration()), masks.len()));
        for (s, e) in &seg.pieces {
            r.push_str(&format!("  側錄 {} – {}\n", tc(secs(*s)), tc(secs(*e))));
        }
        let own = set.for_segment(seg.n);
        let seg_set = own.as_ref().unwrap_or(set);
        for m in masks {
            let turns = rotate_mask(seg_set, m, rotate_secs);
            let mut used: Vec<usize> = turns.iter().map(|t| t.still).collect();
            used.sort_unstable();
            used.dedup();
            let rotate = if used.len() > 1 { format!(" ｜ 輪播 {} 張、換 {} 次", used.len(), turns.len() - 1) } else { String::new() };
            let mut why = if m.note == Some(MANUAL_NOTE) {
                String::new()
            } else if m.ai && m.iou == 0.0 && m.by_label && m.note.is_none() {
                "AI 判斷（來源字相同）".to_string()
            } else {
                format!(
                    "{}{}{}",
                    if m.by_label { "來源字" } else { "畫面相似" },
                    if m.by_label { format!("(IoU {:.2})", m.iou) } else { String::new() },
                    if m.ai && m.note != Some(SCENE_NOTE) { "＋AI 接上空檔" } else { "" }
                )
            };
            if let Some(n) = m.note {
                if !why.is_empty() {
                    why.push('＋');
                }
                why.push_str(n);
            }
            r.push_str(&format!("    遮 {} – {} ｜ {} ｜ {} ｜ {}{rotate}\n", tc(secs(m.start)), tc(secs(m.end)), m.layout.zh(), why, set.stills[m.still].name));
            if used.len() > 1 {
                for k in used.iter().filter(|&&k| k != m.still) {
                    r.push_str(&format!("      輪播：{}\n", set.stills[*k].name));
                }
            }
        }
    }
    r
}

/// 覆蓋檢查：每張靜圖在影片裡最像的那一格，有沒有被遮到
pub fn report_coverage(frames: &[FrameInfo], segs: &[(Segment, Vec<MaskSeg>)], set: &StillSet) -> String {
    let mut r = String::from("\n== 覆蓋檢查 ==（靜圖｜側錄中最像的位置｜相似度｜狀態）\n");
    for (k, s) in set.stills.iter().enumerate() {
        // 只給某一段的靜圖：只在那段裡找
        let own = |i: usize| s.segment.is_none_or(|n| segs.iter().any(|(g, _)| g.n == n && g.pieces.iter().any(|(a, b)| i >= *a && i < *b)));
        if let Some((i, f)) = frames.iter().enumerate().filter(|(i, _)| own(*i)).max_by(|a, b| a.1.sims[k].total_cmp(&b.1.sims[k])) {
            let in_seg = segs.iter().find(|(g, _)| g.pieces.iter().any(|(a, b)| i >= *a && i < *b));
            let masked = segs.iter().any(|(_, ms)| ms.iter().any(|m| i >= m.start && i < m.end));
            let state = match (in_seg, masked) {
                (None, _) => "不在節目段內".to_string(),
                (Some((g, _)), true) => format!("第 {} 段，已遮", g.n),
                (Some((g, _)), false) => format!("第 {} 段，★沒遮", g.n),
            };
            r.push_str(&format!("  {} ｜ {} ｜ {:.2} ｜ {}\n", s.name, tc(secs(i)), f.sims[k], state));
        }
    }
    r
}

/// 跟人工切點比（開發用）。`gt`：每行「段 開頭秒 結尾秒」
pub fn report_gt(gt: &str, segs: &[Segment]) -> String {
    let mut r = String::from("\n== 跟人工切點比對 ==（差幾格；正＝比人工晚）\n");
    for line in gt.lines() {
        let v: Vec<f64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if v.len() < 3 {
            continue;
        }
        let n = v[0] as usize;
        match segs.iter().find(|s| s.n == n) {
            Some(s) => {
                let (a, b) = (secs(s.start()), secs(s.end()));
                r.push_str(&format!("  第 {n} 段：開頭 {:+.0} 格、結尾 {:+.0} 格\n", (a - v[1]) * FPS, (b - v[2]) * FPS));
            }
            None => r.push_str(&format!("  第 {n} 段：★沒找到\n")),
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autocut::frame::Mask;
    use crate::autocut::stills::{Still, Template};
    use crate::autocut::titles::parse_titles;

    fn frame(program: bool) -> FrameInfo {
        FrameInfo { program, layout: Layout::Full, luma: 100.0, cut_diff: 0.0, tmpl: -1, iou: 0.0, sims: vec![] }
    }

    /// 一張靜圖、一個樣板的最小集合
    fn one_still_set() -> StillSet {
        let mask = Mask::from_fn(4, 4, |_, _| 0.0);
        StillSet {
            stills: vec![Still { name: "s.jpg".into(), path: "/s.jpg".into(), segment: None, layout: Layout::Full, template: Some(0), source: StillSet::build(vec![("x.jpg".into(), "/x.jpg".into(), image::RgbImage::new(crate::autocut::W, crate::autocut::H))]).stills[0].source.clone(), small: GrayImage::new(80, 45) }],
            templates: vec![Template { layout: Layout::Full, mask, stills: vec![0], generic: false, split: None }],
            off: Vec::new(),
            kin: Vec::new(),
        }
    }

    #[test]
    fn stills_of_other_segments_are_ignored() {
        // 10/27：第 2 段 iPhone 的靜圖（翻攝 X）蓋到第 3 段普吉島淹水。放進「第2段」子資料夾後第 3 段完全不比
        let mut set = one_still_set();
        set.stills[0].segment = Some(2);
        let mut f = frame(true);
        f.tmpl = 0;
        f.iou = 0.95;
        // 來源字一樣、畫面只有幾分像（別的素材剛好同一個來源）
        f.sims = vec![0.7];
        let n = 6 * FPS as usize;
        let frames: Vec<FrameInfo> = (0..n).map(|_| f.clone()).collect();
        let seg = |g: usize| Segment { n: g, pieces: vec![(0, n)], start_ok: true, end_ok: true };
        assert_eq!(segment_masks(&frames, &set, &seg(2)).len(), 1, "自己這段照樣遮");
        assert!(segment_masks(&frames, &set, &seg(3)).is_empty(), "別段的靜圖不遮");
        assert!(reused_segments(&frames, &set, &seg(3)).is_empty());
        // 11/06 第 4 段重播第 1 段的 60 Minutes：畫面幾乎一樣＋同一個來源字 → 第 1 段的靜圖第 4 段也拿來比
        let replay: Vec<FrameInfo> = frames.iter().map(|f| FrameInfo { sims: vec![0.95], ..f.clone() }).collect();
        assert_eq!(reused_segments(&replay, &set, &seg(3)).iter().map(|r| r.0).collect::<Vec<_>>(), vec![2]);
        assert_eq!(segment_masks(&replay, &set, &seg(3)).len(), 1, "重播的素材要遮");
        // 只有半秒像：不算重播
        let blip: Vec<FrameInfo> = (0..n).map(|i| if i < FPS as usize / 2 { replay[i].clone() } else { frames[i].clone() }).collect();
        assert!(reused_segments(&blip, &set, &seg(3)).is_empty());
        set.stills[0].segment = None;
        assert_eq!(segment_masks(&frames, &set, &seg(3)).len(), 1, "共用的每段都比");
    }

    #[test]
    fn runs_fills_gaps_and_drops_short() {
        let f = |s: &str| s.chars().map(|c| c == '1').collect::<Vec<_>>();
        assert_eq!(runs(&f("0011100111000"), 2, 1), vec![(2, 10)]);
        assert_eq!(runs(&f("0011100011100"), 2, 1), vec![(2, 5), (8, 11)]);
        assert_eq!(runs(&f("1100000001"), 0, 2), vec![(0, 2)]);
        assert!(runs(&f(""), 2, 1).is_empty());
    }

    #[test]
    fn layout_smoothing() {
        use Layout::*;
        let short = (1.5 * FPS) as usize - 1;
        let mut v: Vec<Layout> = [vec![Full; 50], vec![Dual; short], vec![Full; 50]].concat();
        smooth_layouts(&mut v);
        assert!(v.iter().all(|l| *l == Full), "不到 1.5 秒的抖動抹平");
        let mut v: Vec<Layout> = [vec![Full; 50], vec![Dual; short + 1], vec![Full; 50]].concat();
        smooth_layouts(&mut v);
        assert_eq!(v.iter().filter(|l| **l == Dual).count(), short + 1, "1.5 秒以上的保留");
        let mut v: Vec<Layout> = [vec![Dual; 5], vec![Full; 50]].concat();
        smooth_layouts(&mut v);
        assert_eq!(v[0], Dual, "開頭沒有前一段，不動");
    }

    #[test]
    fn one_line_label_needs_picture_too() {
        // 10/27：普吉島淹水的「翻攝 X」跟 iPhone 新聞靜圖的「翻攝 X」字形一樣，畫面完全不像
        let mut set = one_still_set();
        set.templates[0].generic = true;
        let mut f = frame(true);
        f.tmpl = 0;
        f.iou = 0.95;
        f.sims = vec![0.1];
        assert!(!label_hit(&f, &set), "只有一行的來源字，字形再像也不能單獨遮");
        f.sims = vec![0.7];
        assert!(!label_hit(&f, &set), "畫面有幾分像也不算（10/27 淹水畫面一瞬間 0.70）");
        // 真的是那支影片：畫面跟靜圖幾乎一樣，靠畫面相似（plan_masks 的 SIM_NCC）照樣遮
        f.sims = vec![SIM_NCC];
        let n = 4 * FPS as usize;
        let frames: Vec<FrameInfo> = (0..n).map(|_| f.clone()).collect();
        assert_eq!(plan_masks(&frames, &set, (0, n)).len(), 1, "畫面幾乎一樣照樣遮");
    }

    #[test]
    fn label_hit_rules() {
        let set = one_still_set();
        let mut f = frame(true);
        f.sims = vec![0.0];
        f.tmpl = 0;
        f.iou = 0.62;
        assert!(label_hit(&f, &set), "≥0.62 直接命中");
        f.iou = 0.5;
        assert!(!label_hit(&f, &set), "0.45～0.62 畫面不像不算");
        f.sims = vec![0.6];
        assert!(label_hit(&f, &set), "0.45～0.62 加上畫面 NCC ≥0.6");
        f.iou = 0.44;
        assert!(!label_hit(&f, &set));
        f.tmpl = -1;
        f.iou = 0.9;
        assert!(!label_hit(&f, &set), "沒有樣板");
    }

    /// 第三段只是讓第二段不是最後一段（最後一段的結尾規則不一樣，另外測）
    const TITLES: &str = "第一段\n川普關稅大棒再揮向盟友\n=======\n第二段\n伊朗忍者部隊潛入以色列境內\n=======\n第三段\n中國人口將跌破十四億失業仍暴增\n";

    /// 時間軸：廣告 a 秒 → 節目 b 秒 → 廣告 c 秒 …，OCR 每 2 秒一張，節目裡填 `seg` 段的標題
    fn timeline(parts: &[(bool, usize, usize)]) -> (Vec<FrameInfo>, BTreeMap<usize, String>) {
        let fps = FPS as usize;
        let mut frames = Vec::new();
        let mut ocr = BTreeMap::new();
        let text = |seg: usize| ["", "川普關稅大棒再揮向盟友", "伊朗忍者部隊潛入以色列境內", "中國人口將跌破十四億失業仍暴增"][seg];
        for &(program, sec, seg) in parts {
            let start = frames.len();
            for i in 0..sec * fps {
                let mut f = frame(program);
                // 每個區塊第一格當成鏡頭切換
                if i == 0 {
                    f.cut_diff = 50.0;
                }
                frames.push(f);
            }
            if program {
                for k in (start..frames.len()).step_by(2 * fps) {
                    ocr.insert(k, text(seg).to_string());
                }
            }
        }
        (frames, ocr)
    }

    #[test]
    fn segments_complete_incomplete_and_rerun() {
        let titles = parse_titles(TITLES);
        let (frames, ocr) = timeline(&[(false, 10, 0), (true, 60, 1), (false, 10, 0), (true, 40, 2), (false, 10, 0), (true, 20, 2), (false, 10, 0)]);
        let mut log = String::new();
        let segs = plan_segments(&frames, &ocr, &titles, &mut log);
        assert_eq!(segs.len(), 2);
        let fps = FPS as usize;
        assert_eq!(segs[0].n, 1);
        assert_eq!(segs[0].start(), 10 * fps);
        assert_eq!(segs[0].end(), 70 * fps - END_TRIM);
        assert!(segs[0].complete());
        assert_eq!(segs[1].n, 2);
        assert_eq!(segs[1].pieces.len(), 1, "後面又出現的第 2 段（預告／重播）不收");
        assert!(log.contains("又出現（預告／重播），不收"), "{log}");

        // 素材斷在第 2 段中間 → 開頭完整、結尾不完整（監看時＝錄製中）
        let cut = 10 + 60 + 10 + 30;
        let partial = &frames[..cut * fps];
        let ocr2: BTreeMap<usize, String> = ocr.range(..cut * fps).map(|(k, v)| (*k, v.clone())).collect();
        let segs = plan_segments(partial, &ocr2, &titles, &mut String::new());
        assert!(segs[0].complete());
        assert!(segs[1].start_ok && !segs[1].end_ok);
        // 結尾後才剛進廣告 3 秒（不到 5 秒）也還不算完整
        let cut = 10 + 60 + 10 + 40 + 3;
        let segs = plan_segments(&frames[..cut * fps], &ocr, &titles, &mut String::new());
        assert!(!segs[1].end_ok);
        let cut = 10 + 60 + 10 + 40 + 5;
        let segs = plan_segments(&frames[..cut * fps], &ocr, &titles, &mut String::new());
        assert!(segs[1].end_ok);

        // 素材從節目中間開始 → 開頭不完整
        let segs = plan_segments(&frames[20 * fps..], &BTreeMap::new(), &titles, &mut String::new());
        assert!(segs.is_empty(), "沒有 OCR 對到文稿的區塊略過");
        let ocr3: BTreeMap<usize, String> = ocr.iter().filter(|(k, _)| **k >= 20 * fps).map(|(k, v)| (k - 20 * fps, v.clone())).collect();
        let segs = plan_segments(&frames[20 * fps..], &ocr3, &titles, &mut String::new());
        assert!(!segs[0].start_ok);
    }

    #[test]
    fn clock_ends_last_segment_only_past_cutoff() {
        assert_eq!(parse_clock("23:59:30"), Some(86370));
        assert_eq!(parse_clock("23：59"), Some(86340));
        assert_eq!(parse_clock(""), None);
        assert_eq!(parse_clock("25:00"), None);
        let fps = FPS as usize;
        let cutoff = 86370;
        // 節目到第 60 秒，之後非節目（中間廣告或收尾，亮度不黑）
        let (frames, _) = timeline(&[(true, 60, 3), (false, 180, 0)]);
        let e = 60 * fps;
        assert!(!clock_end_ok(&frames, e, 86369.0, cutoff), "還沒到時間：可能是中間廣告，要等");
        assert!(clock_end_ok(&frames, e, 86370.0, cutoff), "過了時間、節目沒回來＝結束");
        assert!(clock_end_ok(&frames, e, 86440.0, cutoff), "錄過午夜也算");
        assert!(!clock_end_ok(&frames[..e + 3 * fps], e, 86440.0, cutoff), "標題條消失才 3 秒，可能只是素材斷在節目中間");
        let (on_air, _) = timeline(&[(true, 240, 3)]);
        assert!(!clock_end_ok(&on_air, e, 86440.0, cutoff), "節目還在播（超時）不算");
    }

    #[test]
    fn last_segment_joins_after_ads_and_waits_for_black() {
        // 10/27 第 6 段：播 → 廣告 → 又播 → 廣告 → 收黑
        let titles = parse_titles(TITLES);
        let fps = FPS as usize;
        let (mut frames, ocr) = timeline(&[(false, 10, 0), (true, 60, 2), (false, 10, 0), (true, 60, 3), (false, 270, 0), (true, 80, 3), (false, 10, 0), (false, 60, 0)]);
        let black = (10 + 60 + 10 + 60 + 270 + 80 + 10) * fps;
        frames[black..].iter_mut().for_each(|f| f.luma = 16.0);
        let mut log = String::new();
        let segs = plan_segments(&frames, &ocr, &titles, &mut log);
        let last = segs.iter().find(|s| s.n == 3).unwrap();
        assert_eq!(last.pieces.len(), 2, "廣告後回來的接在後面：{log}");
        assert_eq!(last.pieces[0], (80 * fps, 140 * fps - END_TRIM), "第一片結尾也提早收");
        assert_eq!(last.pieces[1], (410 * fps, 490 * fps - END_TRIM));
        assert!(piece_after_break(last, 0) && piece_after_break(last, 1));
        assert!(last.complete());

        // 第一次進廣告就停在這裡：還沒收黑，不能算完整（以前會在這裡就出片）
        let cut = (10 + 60 + 10 + 60 + 30) * fps;
        let ocr2: BTreeMap<usize, String> = ocr.range(..cut).map(|(k, v)| (*k, v.clone())).collect();
        let segs = plan_segments(&frames[..cut], &ocr2, &titles, &mut String::new());
        let last = segs.iter().find(|s| s.n == 3).unwrap();
        assert!(last.start_ok && !last.end_ok);
        // 收黑還不到 5 秒也還不算
        let segs = plan_segments(&frames[..black + 4 * fps], &ocr, &titles, &mut String::new());
        assert!(!segs.iter().find(|s| s.n == 3).unwrap().end_ok);
        // 沒收黑（2400 直接接別的節目）：結尾後錄滿 8 分鐘也算結束
        frames.iter_mut().for_each(|f| f.luma = 100.0);
        frames.extend((0..LAST_GAP_SECS as usize * fps).map(|_| frame(false)));
        assert!(plan_segments(&frames, &ocr, &titles, &mut String::new()).iter().find(|s| s.n == 3).unwrap().complete());
    }

    #[test]
    fn earlier_segment_reappearing_is_not_joined() {
        // 第 1 段在後面又出現（預告／重播）：只有最後一段會接回來
        let titles = parse_titles(TITLES);
        let (frames, ocr) = timeline(&[(false, 10, 0), (true, 60, 1), (false, 10, 0), (true, 40, 1), (false, 10, 0)]);
        let segs = plan_segments(&frames, &ocr, &titles, &mut String::new());
        assert_eq!(segs[0].pieces.len(), 1);
    }

    #[test]
    fn split_inside_block_without_ads() {
        let titles = parse_titles(TITLES);
        let fps = FPS as usize;
        let (mut frames, mut ocr) = timeline(&[(false, 10, 0), (true, 60, 1), (true, 60, 2), (false, 10, 0)]);
        // 兩段中間沒有廣告：timeline 會在 70 秒處放鏡頭切換，OCR 換段點對齊到那裡
        let segs = plan_segments(&frames, &ocr, &titles, &mut String::new());
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].end(), 70 * fps - END_TRIM);
        assert_eq!(segs[1].start(), 70 * fps);
        assert!(segs[0].complete() && segs[1].complete(), "換段點算完整");
        // 雜訊：區塊裡只出現 2 次的段落不算
        frames.truncate(74 * fps);
        ocr.retain(|k, _| *k < 74 * fps);
        let segs = plan_segments(&frames, &ocr, &titles, &mut String::new());
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn logo_end_finds_dissolve_finish() {
        let logo = GrayImage::from_fn(80, 45, |x, y| image::Luma([((x * 7 + y * 3) % 200) as u8]));
        let studio = GrayImage::from_fn(80, 45, |x, y| image::Luma([((y * 11 + x) % 180) as u8]));
        let frames: Vec<GrayImage> = [vec![studio.clone(); 5], vec![logo.clone(); 10], vec![studio.clone(); 5]].concat();
        assert_eq!(logo_end(&logo, &frames), Some(15));
        assert_eq!(logo_end(&logo, &[studio.clone(), studio]), None, "沒看到 Logo");
        assert_eq!(refine_window(1000), (1000 - 179, 179 + 58));
        assert_eq!(refine_window(10), (0, 10 + 58));
    }

    #[test]
    fn masks_from_label_hits() {
        let set = one_still_set();
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..60 * fps)
            .map(|_| {
                let mut f = frame(true);
                f.sims = vec![0.1];
                f
            })
            .collect();
        // 20～30 秒來源字命中；中間 1 秒斷掉（≤2 秒補起來）；19 秒處有鏡頭切換（往前延伸到那裡）
        for f in &mut frames[20 * fps..30 * fps] {
            f.tmpl = 0;
            f.iou = 0.8;
        }
        for f in &mut frames[24 * fps..25 * fps] {
            f.tmpl = -1;
        }
        frames[19 * fps].cut_diff = 40.0;
        let m = plan_masks(&frames, &set, (0, frames.len()));
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].start, 19 * fps);
        assert_eq!(m[0].end, 30 * fps);
        assert!(m[0].by_label);
        assert!((m[0].iou - 0.8).abs() < 1e-6);
        // 範圍外的不遮
        assert!(plan_masks(&frames, &set, (0, 10 * fps)).is_empty());
    }

    #[test]
    fn whole_shot_is_masked_and_flash_is_not_a_cut() {
        // 10/28 第 3 段攤販監視器：鏡頭 10～24 秒，只有 12～14 秒跟截圖幾乎一樣；18 秒爆炸閃光（亮度暴衝又回來）
        let set = one_still_set();
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..40 * fps)
            .map(|_| {
                let mut f = frame(true);
                f.sims = vec![0.3];
                f
            })
            .collect();
        frames[10 * fps].cut_diff = 60.0;
        frames[24 * fps].cut_diff = 60.0;
        // 整個鏡頭都看得到同一個來源字（字形 0.42：不夠單獨當證據，夠確認還在同一支影片）
        for f in &mut frames[10 * fps..24 * fps] {
            f.tmpl = 0;
            f.iou = 0.42;
        }
        for f in &mut frames[12 * fps..14 * fps] {
            f.sims = vec![0.95];
        }
        frames[18 * fps].cut_diff = 45.0;
        for f in &mut frames[18 * fps..18 * fps + 5] {
            f.luma = 200.0;
        }
        frames[18 * fps + 5].cut_diff = 45.0;
        assert!(!is_shot_cut(&frames, 18 * fps), "閃光不算換鏡頭");
        assert!(is_shot_cut(&frames, 24 * fps));
        let m = plan_masks(&frames, &set, (0, frames.len()));
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!((m[0].start, m[0].end), (10 * fps, 24 * fps), "整個鏡頭都遮，包含爆炸那幾秒");
        assert!(!m[0].by_label);
    }

    #[test]
    fn whole_shot_stops_where_the_source_label_disappears() {
        // 10/29 第 5 段 1:04–1:14：棚內電子板（沒有來源字）10 秒慢慢放大到素材，中間沒有硬切；
        // 素材（有來源字）20～25 秒，21 秒跟截圖一樣 → 電子板不遮
        let set = one_still_set();
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..40 * fps)
            .map(|_| {
                let mut f = frame(true);
                f.sims = vec![0.2];
                f
            })
            .collect();
        frames[10 * fps].cut_diff = 60.0;
        frames[25 * fps].cut_diff = 60.0;
        for f in &mut frames[20 * fps..25 * fps] {
            f.tmpl = 0;
            f.iou = 0.5;
        }
        for f in &mut frames[21 * fps..22 * fps] {
            f.sims = vec![0.95];
        }
        let m = plan_masks(&frames, &set, (0, frames.len()));
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!((m[0].start, m[0].end), (20 * fps, 25 * fps), "只遮有來源字的部分");
        // 素材沒有來源字（靜圖也沒有樣板）→ 照舊整個鏡頭遮
        let mut set = set;
        set.stills[0].template = None;
        let m = plan_masks(&frames, &set, (0, frames.len()));
        assert_eq!((m[0].start, m[0].end), (10 * fps, 25 * fps));
    }

    #[test]
    fn same_still_masks_close_together_merge() {
        // 10/28 電工：同一支影片中間夾一個不像的鏡頭（5 秒）→ 接起來（靜圖沒有來源字：整個鏡頭遮不受來源字限制）
        let mut set = one_still_set();
        set.stills[0].template = None;
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..40 * fps)
            .map(|_| {
                let mut f = frame(true);
                f.sims = vec![0.1];
                f
            })
            .collect();
        for k in [10, 14, 19, 23] {
            frames[k * fps].cut_diff = 60.0;
        }
        for i in (11 * fps..12 * fps).chain(20 * fps..21 * fps) {
            frames[i].sims = vec![0.95];
        }
        let m = plan_masks(&frames, &set, (0, frames.len()));
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!((m[0].start, m[0].end), (10 * fps, 23 * fps));
        // 隔太遠（> 8 秒）不接
        for f in &mut frames[20 * fps..21 * fps] {
            f.sims = vec![0.1];
        }
        frames[33 * fps].cut_diff = 60.0;
        frames[36 * fps].cut_diff = 60.0;
        for f in &mut frames[34 * fps..35 * fps] {
            f.sims = vec![0.95];
        }
        assert_eq!(plan_masks(&frames, &set, (0, frames.len())).len(), 2);
    }

    /// 10/20 第 2 段 8:42–9:00：雙框特寫（像靜圖）有遮、中間遠景（不像、來源字比不到）沒遮，
    /// 前後是同一張靜圖 → 空檔交給 AI；AI 說同一個來源就接成一段
    #[test]
    fn gap_between_same_source_masks_goes_to_ai() {
        let set = one_still_set();
        let fps = FPS as usize;
        let n = 120 * fps;
        let frames: Vec<FrameInfo> = (0..n).map(|_| FrameInfo { program: true, layout: Layout::Dual, luma: 100.0, cut_diff: 0.0, tmpl: -1, iou: 0.0, sims: vec![0.2] }).collect();
        let seg = Segment { n: 2, pieces: vec![(0, n)], start_ok: true, end_ok: true };
        let m = |a: usize, b: usize| MaskSeg { start: a * fps, end: b * fps, layout: Layout::Dual, still: 0, by_label: false, iou: 0.0, ai: false, note: None };
        let mut masks = vec![m(20, 25), m(43, 50)];
        let c = ai_candidates(&frames, &set, &seg, &masks);
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!((c[0].start, c[0].end, c[0].kind), (25 * fps, 43 * fps, AiKind::Gap));
        assert_eq!(c[0].stills, vec![0]);
        assert_eq!(c[0].samples.len(), 3);
        assert!(c[0].samples.iter().all(|&s| s > 25 * fps && s < 43 * fps));
        apply_ai(&frames, &mut masks, &c[0]);
        assert_eq!(masks.len(), 1);
        assert_eq!((masks[0].start, masks[0].end), (20 * fps, 50 * fps));
        assert!(masks[0].ai);
        // 空檔超過 60 秒、或版型不同就不問
        let far = vec![m(0, 5), m(70, 75)];
        assert!(ai_candidates(&frames, &set, &seg, &far).is_empty());
    }

    #[test]
    fn weak_scores_go_to_ai_and_become_a_mask() {
        let set = one_still_set();
        let fps = FPS as usize;
        let n = 60 * fps;
        let mut frames: Vec<FrameInfo> = (0..n).map(|_| FrameInfo { program: true, layout: Layout::Full, luma: 100.0, cut_diff: 0.0, tmpl: -1, iou: 0.0, sims: vec![0.1] }).collect();
        // 10～14 秒：來源字分數 0.3（灰色地帶）
        for f in &mut frames[10 * fps..14 * fps] {
            f.tmpl = 0;
            f.iou = 0.3;
        }
        let seg = Segment { n: 1, pieces: vec![(0, n)], start_ok: true, end_ok: true };
        let mut masks = Vec::new();
        let c = ai_candidates(&frames, &set, &seg, &masks);
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!((c[0].start, c[0].end, c[0].kind), (10 * fps, 14 * fps, AiKind::Weak));
        apply_ai(&frames, &mut masks, &c[0]);
        assert_eq!(masks.len(), 1);
        assert!(masks[0].ai && masks[0].by_label);
        // 分數很低（< 0.15）不問
        for f in &mut frames[10 * fps..14 * fps] {
            f.iou = 0.1;
        }
        assert!(ai_candidates(&frames, &set, &seg, &[]).is_empty());
        // 灰色地帶太短（< 2 秒）不問
        for f in &mut frames[10 * fps..11 * fps] {
            f.iou = 0.3;
        }
        assert!(ai_candidates(&frames, &set, &seg, &[]).is_empty());
    }

    /// 灰色地帶很多時，空檔還是一定會問（不會被次數上限擠掉）
    #[test]
    fn gaps_are_never_dropped_by_the_weak_limit() {
        let set = one_still_set();
        let fps = FPS as usize;
        let n = 300 * fps;
        let mut frames: Vec<FrameInfo> = (0..n).map(|_| FrameInfo { program: true, layout: Layout::Dual, luma: 100.0, cut_diff: 0.0, tmpl: -1, iou: 0.0, sims: vec![0.1] }).collect();
        // 前 200 秒每 6 秒有一段 3 秒的灰色地帶（共 33 處，超過上限）
        for k in 0..33 {
            for f in &mut frames[k * 6 * fps..(k * 6 + 3) * fps] {
                f.tmpl = 0;
                f.iou = 0.3;
            }
        }
        let seg = Segment { n: 2, pieces: vec![(0, n)], start_ok: true, end_ok: true };
        let m = |a: usize, b: usize| MaskSeg { start: a * fps, end: b * fps, layout: Layout::Dual, still: 0, by_label: false, iou: 0.0, ai: false, note: None };
        let masks = vec![m(250, 255), m(270, 280)];
        let c = ai_candidates(&frames, &set, &seg, &masks);
        assert_eq!(c.iter().filter(|c| c.kind == AiKind::Weak).count(), AI_MAX_WEAK_PER_SEGMENT);
        assert!(c.iter().any(|c| c.kind == AiKind::Gap && c.start == 255 * fps), "{c:?}");
    }

    fn mask(a: usize, b: usize, layout: Layout, still: usize) -> MaskSeg {
        let fps = FPS as usize;
        MaskSeg { start: a * fps, end: b * fps, layout, still, by_label: false, iou: 0.0, ai: false, note: None }
    }

    /// 11/02 第 2 段 3:52–3:56：滿版遮罩、4 秒沒遮的滿版近景、雙框遮罩 → 近景接到前面的滿版
    #[test]
    fn sandwich_gap_between_full_and_dual_is_filled() {
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..60 * fps).map(|_| frame(true)).collect();
        for f in &mut frames[24 * fps..] {
            f.layout = Layout::Dual;
        }
        let m = fill_sandwich_gaps(&frames, &[mask(10, 20, Layout::Full, 0), mask(24, 30, Layout::Dual, 1)]);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!((m[0].start, m[0].end, m[0].layout), (10 * fps, 24 * fps, Layout::Full));
        assert_eq!(m[0].note, Some(SANDWICH_NOTE));
        assert_eq!((m[1].start, m[1].end), (24 * fps, 30 * fps));
        // 空檔是雙框 → 接到後面的雙框
        for f in &mut frames[20 * fps..24 * fps] {
            f.layout = Layout::Dual;
        }
        let m = fill_sandwich_gaps(&frames, &[mask(10, 20, Layout::Full, 0), mask(24, 30, Layout::Dual, 1)]);
        assert_eq!((m[0].end, m[1].start), (20 * fps, 20 * fps), "{m:?}");
        // 前後都是滿版、不同靜圖 → 中間接到前一段
        let frames: Vec<FrameInfo> = (0..60 * fps).map(|_| frame(true)).collect();
        let m = fill_sandwich_gaps(&frames, &[mask(10, 20, Layout::Full, 0), mask(25, 30, Layout::Full, 1)]);
        assert_eq!((m[0].end, m[1].start, m[1].still), (25 * fps, 25 * fps, 1));
    }

    #[test]
    fn long_gap_or_non_program_is_not_filled() {
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..60 * fps).map(|_| frame(true)).collect();
        let far = [mask(10, 20, Layout::Full, 0), mask(29, 35, Layout::Full, 1)];
        assert_eq!(fill_sandwich_gaps(&frames, &far), far.to_vec(), "超過 8 秒不補");
        for f in &mut frames[20 * fps..24 * fps] {
            f.program = false;
        }
        let near = [mask(10, 20, Layout::Full, 0), mask(25, 30, Layout::Full, 1)];
        assert_eq!(fill_sandwich_gaps(&frames, &near), near.to_vec(), "中間不是節目畫面（廣告）不補");
    }

    #[test]
    fn segment_masks_fill_sandwich_gaps() {
        // 兩支不同靜圖的滿版素材中間隔 4 秒：以前會露出來，現在補起來
        let mut set = one_still_set();
        set.stills[0].template = None;
        let mut s2 = set.stills[0].clone();
        s2.name = "t.jpg".into();
        set.stills.push(s2);
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..60 * fps)
            .map(|_| {
                let mut f = frame(true);
                f.sims = vec![0.1, 0.1];
                f
            })
            .collect();
        for k in [10, 20, 24, 30] {
            frames[k * fps].cut_diff = 60.0;
        }
        for f in &mut frames[12 * fps..13 * fps] {
            f.sims = vec![0.95, 0.1];
        }
        for f in &mut frames[26 * fps..27 * fps] {
            f.sims = vec![0.1, 0.95];
        }
        let seg = Segment { n: 2, pieces: vec![(0, frames.len())], start_ok: true, end_ok: true };
        let m = segment_masks(&frames, &set, &seg);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!((m[0].start, m[0].end, m[1].start, m[1].end), (10 * fps, 24 * fps, 24 * fps, 30 * fps));
    }

    /// 11/02 第 2 段 8:25：滿版遮罩前後緊接的鏡頭（來源字不同）→ 問 AI 是不是同一場；說是就接上
    #[test]
    fn neighbor_shots_of_full_masks_go_to_ai() {
        let set = one_still_set();
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..120 * fps).map(|_| frame(true)).collect();
        for k in [17, 20, 25, 28] {
            frames[k * fps].cut_diff = 60.0;
        }
        let seg = Segment { n: 2, pieces: vec![(0, frames.len())], start_ok: true, end_ok: true };
        let mut masks = vec![mask(20, 25, Layout::Full, 0)];
        let c = ai_candidates(&frames, &set, &seg, &masks);
        let scene: Vec<(usize, usize)> = c.iter().filter(|c| c.kind == AiKind::Scene).map(|c| (c.start, c.end)).collect();
        assert_eq!(scene, vec![(17 * fps, 20 * fps), (25 * fps, 28 * fps)], "{c:?}");
        for c in c.iter().filter(|c| c.kind == AiKind::Scene) {
            assert_eq!(c.stills, vec![0]);
            apply_ai(&frames, &mut masks, c);
        }
        assert_eq!(masks.len(), 1);
        assert_eq!((masks[0].start, masks[0].end), (17 * fps, 28 * fps));
        assert_eq!(masks[0].note, Some(SCENE_NOTE));
        // 雙框不問；後面沒有鏡頭切換 → 最多看 15 秒
        let dual = vec![mask(20, 25, Layout::Dual, 0)];
        assert!(ai_candidates(&frames, &set, &seg, &dual).iter().all(|c| c.kind != AiKind::Scene));
        // 兩段同一張靜圖中間 10 秒（超過夾心 8 秒）→ 問空檔；中間 33 秒有換鏡頭 → 前後兩個鏡頭也各問一次相鄰鏡頭，空檔先處理
        let gap = vec![mask(25, 30, Layout::Full, 0), mask(40, 45, Layout::Full, 0)];
        frames[33 * fps].cut_diff = 60.0;
        let c = ai_candidates(&frames, &set, &seg, &gap);
        let at = |s: usize| c.iter().filter(|c| c.start == s * fps).map(|c| c.kind).collect::<Vec<_>>();
        assert_eq!(at(30), vec![AiKind::Gap, AiKind::Scene], "{c:?}");
        assert!(c.iter().any(|c| c.kind == AiKind::Scene && (c.start, c.end) == (33 * fps, 40 * fps)), "{c:?}");
        // 空檔被判不同、相鄰鏡頭被判同一場 → 只接那個鏡頭
        let mut m2 = gap.clone();
        let scene = c.iter().find(|c| c.kind == AiKind::Scene && c.start == 30 * fps).unwrap();
        apply_ai(&frames, &mut m2, scene);
        assert_eq!((m2[0].end, m2[1].start), (33 * fps, 40 * fps));
        // 空檔判同一個來源 → 整段接起來，之後的相鄰鏡頭不再動
        let mut m3 = gap.clone();
        for c in c.iter().filter(|c| c.start >= 30 * fps && c.end <= 40 * fps) {
            apply_ai(&frames, &mut m3, c);
        }
        assert_eq!(m3.len(), 1);
        assert_eq!((m3[0].start, m3[0].end), (25 * fps, 45 * fps));
        // 前後沒有鏡頭切換：前一段往後、後一段往前會是同一塊，只問一次
        frames[33 * fps].cut_diff = 0.0;
        let c = ai_candidates(&frames, &set, &seg, &gap);
        let inside: Vec<_> = c.iter().filter(|c| c.kind == AiKind::Scene && c.start >= 30 * fps && c.end <= 40 * fps).collect();
        assert_eq!(inside.len(), 1, "{c:?}");
        assert_eq!((inside[0].start, inside[0].end), (30 * fps, 40 * fps));
        let late = vec![mask(60, 70, Layout::Full, 0)];
        let c = ai_candidates(&frames, &set, &seg, &late);
        let lim = (SCENE_MAX_SECS * FPS) as usize;
        assert!(c.iter().any(|c| c.kind == AiKind::Scene && (c.start, c.end) == (70 * fps, 70 * fps + lim)), "{c:?}");
    }

    /// 手動遮罩：一定遮，重疊的原本遮罩切掉重疊那截；沒指定靜圖就挑這段平均最像的
    #[test]
    fn manual_mask_overrides_and_picks_still() {
        let fps = FPS as usize;
        let mut masks = vec![mask(10, 20, Layout::Full, 0), mask(30, 40, Layout::Full, 0)];
        apply_manual(&mut masks, 15 * fps, 35 * fps, 0, Layout::Dual);
        let got: Vec<(usize, usize, Option<&str>)> = masks.iter().map(|m| (m.start / fps, m.end / fps, m.note)).collect();
        assert_eq!(got, vec![(10, 15, None), (15, 35, Some(MANUAL_NOTE)), (35, 40, None)]);
        assert_eq!(masks[1].layout, Layout::Dual);
        // 整段蓋在一段遮罩裡面 → 那段拆成前後兩截
        let mut inside = vec![mask(10, 40, Layout::Full, 0)];
        apply_manual(&mut inside, 20 * fps, 25 * fps, 0, Layout::Full);
        assert_eq!(inside.iter().map(|m| (m.start / fps, m.end / fps)).collect::<Vec<_>>(), vec![(10, 20), (20, 25), (25, 40)]);

        let mut set = one_still_set();
        let mut s2 = set.stills[0].clone();
        s2.name = "b.jpg".into();
        s2.template = None;
        set.stills.push(s2);
        let mut frames: Vec<FrameInfo> = (0..60 * fps)
            .map(|_| {
                let mut f = frame(true);
                f.sims = vec![0.2, 0.1];
                f
            })
            .collect();
        for f in &mut frames[20 * fps..30 * fps] {
            f.sims = vec![0.1, 0.4];
        }
        let seg = Segment { n: 1, pieces: vec![(0, frames.len())], start_ok: true, end_ok: true };
        assert_eq!(pick_still(&frames, &set, &seg, 20 * fps, 30 * fps), Some(1));
        assert_eq!(pick_still(&frames, &set, &seg, 0, 10 * fps), Some(0));
    }

    /// 11/06 第 1 段 12:47：AI 接上後面一個鏡頭後，再後面那個鏡頭要變成新的題目（呼叫端一輪一輪問到不再變）
    #[test]
    fn scene_extension_exposes_next_shot() {
        let set = one_still_set();
        let fps = FPS as usize;
        let mut frames: Vec<FrameInfo> = (0..120 * fps).map(|_| frame(true)).collect();
        for k in [25, 31, 43] {
            frames[k * fps].cut_diff = 60.0;
        }
        let seg = Segment { n: 1, pieces: vec![(0, frames.len())], start_ok: true, end_ok: true };
        let mut masks = vec![mask(10, 20, Layout::Full, 0)];
        let after = |m: &[MaskSeg], s: usize| ai_candidates(&frames, &set, &seg, m).into_iter().find(|c| c.kind == AiKind::Scene && c.start == s * fps);
        let c1 = after(&masks, 20).unwrap();
        assert_eq!(c1.end, 25 * fps);
        apply_ai(&frames, &mut masks, &c1);
        let c2 = after(&masks, 25).expect("接上之後，緊接的下一個鏡頭也要問");
        assert_eq!(c2.end, 31 * fps);
        apply_ai(&frames, &mut masks, &c2);
        assert_eq!((masks[0].start, masks[0].end), (10 * fps, 31 * fps));
    }
}
