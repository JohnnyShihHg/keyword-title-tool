//! 每個側錄檔一份掃描快取（v3）與 OCR 結果。讀寫檔案在 app 端，這裡只管格式。
//!
//! 位置：`{輸出資料夾}\.autocut\{側錄檔名}.cache`，旁邊是 `{側錄檔名}.ocr.tsv`。
//! 開頭記格式代號、側錄檔大小、格數、靜圖簽名；側錄檔大小或靜圖簽名變了就不能沿用（重掃）。
//! OCR 只跟側錄有關、跟靜圖無關，靜圖改了不用重跑。

use super::{FrameInfo, Layout};
use std::collections::BTreeMap;

const MAGIC: &[u8; 4] = b"MDC3";

pub fn encode(file_size: u64, signature: &str, n_stills: usize, frames: &[FrameInfo]) -> Vec<u8> {
    let rec = record_len(n_stills);
    let mut b: Vec<u8> = Vec::with_capacity(24 + signature.len() + frames.len() * rec);
    b.extend(MAGIC);
    b.extend(file_size.to_le_bytes());
    b.extend((frames.len() as u32).to_le_bytes());
    b.extend((n_stills as u32).to_le_bytes());
    b.extend((signature.len() as u32).to_le_bytes());
    b.extend(signature.as_bytes());
    for f in frames {
        b.push(f.program as u8);
        b.push((f.layout == Layout::Dual) as u8);
        b.extend(f.luma.to_le_bytes());
        b.extend(f.cut_diff.to_le_bytes());
        b.extend(f.tmpl.to_le_bytes());
        b.extend(f.iou.to_le_bytes());
        for k in 0..n_stills {
            b.extend(f.sims.get(k).copied().unwrap_or(0.0).to_le_bytes());
        }
    }
    b
}

fn record_len(n_stills: usize) -> usize {
    2 + 4 + 4 + 2 + 4 + 4 * n_stills
}

/// 讀快取；側錄檔大小、靜圖簽名跟現在不一樣就回傳原因（呼叫端改成重掃）
pub fn decode(b: &[u8], file_size: u64, signature: &str) -> Result<Vec<FrameInfo>, String> {
    let bad = || "快取檔損壞".to_string();
    if b.len() < 24 || &b[..4] != MAGIC {
        return Err("不是 v3 快取".into());
    }
    let rd32 = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let size = u64::from_le_bytes(b[4..12].try_into().unwrap());
    if size != file_size {
        return Err(format!("側錄檔大小變了（快取 {size}、現在 {file_size}）"));
    }
    let (n, ns, sig_len) = (rd32(12), rd32(16), rd32(20));
    let head = 24 + sig_len;
    if b.len() < head {
        return Err(bad());
    }
    if &b[24..head] != signature.as_bytes() {
        return Err("靜圖改過".into());
    }
    let rec = record_len(ns);
    if b.len() != head + n * rec {
        return Err(bad());
    }
    let rdf = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    Ok((0..n)
        .map(|i| {
            let o = head + i * rec;
            FrameInfo {
                program: b[o] == 1,
                layout: if b[o + 1] == 1 { Layout::Dual } else { Layout::Full },
                luma: rdf(o + 2),
                cut_diff: rdf(o + 6),
                tmpl: i16::from_le_bytes(b[o + 10..o + 12].try_into().unwrap()),
                iou: rdf(o + 12),
                sims: (0..ns).map(|k| rdf(o + 16 + 4 * k)).collect(),
            }
        })
        .collect())
}

/// OCR 結果：每行「這個檔裡第幾格<Tab>文字」
pub fn encode_ocr(ocr: &BTreeMap<usize, String>) -> String {
    ocr.iter().map(|(f, t)| format!("{f}\t{}\n", t.replace(['\t', '\n', '\r'], " "))).collect()
}

pub fn decode_ocr(s: &str) -> BTreeMap<usize, String> {
    s.lines()
        .filter_map(|l| {
            let (f, t) = l.split_once('\t')?;
            Some((f.trim().parse().ok()?, t.to_string()))
        })
        .collect()
}

/// 側錄檔名排序用的時間：`...---2026_10_20---22_00_40---...` → `20261020220040`。
/// 依檔名時間排序，不用檔案修改時間（網路磁碟的時間不可靠）。
pub fn name_timestamp(name: &str) -> Option<String> {
    let parts: Vec<&str> = name.split("---").collect();
    parts.windows(2).find_map(|w| {
        let d: Vec<&str> = w[0].trim().split('_').collect();
        let t: Vec<&str> = w[1].trim().split('_').collect();
        let digits = |v: &[&str], lens: &[usize]| v.len() == lens.len() && v.iter().zip(lens).all(|(s, l)| s.len() == *l && s.chars().all(|c| c.is_ascii_digit()));
        (digits(&d, &[4, 2, 2]) && digits(&t, &[2, 2, 2])).then(|| format!("{}{}", d.concat(), t.concat()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames() -> Vec<FrameInfo> {
        vec![
            FrameInfo { program: true, layout: Layout::Dual, luma: 12.5, cut_diff: 0.0, tmpl: 3, iou: 0.77, sims: vec![0.1, 0.9] },
            FrameInfo { program: false, layout: Layout::Full, luma: 200.0, cut_diff: 31.0, tmpl: -1, iou: 0.0, sims: vec![0.0, -0.2] },
        ]
    }

    #[test]
    fn roundtrip_and_invalidation() {
        let b = encode(1234, "a.jpg|false|0", 2, &frames());
        assert_eq!(decode(&b, 1234, "a.jpg|false|0").unwrap(), frames());
        assert!(decode(&b, 1235, "a.jpg|false|0").unwrap_err().contains("大小"));
        assert!(decode(&b, 1234, "a.jpg|true|0").unwrap_err().contains("靜圖"));
        assert!(decode(&b[..b.len() - 1], 1234, "a.jpg|false|0").is_err());
        assert!(decode(b"MDC2xxxx", 1234, "").is_err());
        // 沒有靜圖也能存
        let b = encode(1, "", 0, &[]);
        assert!(decode(&b, 1, "").unwrap().is_empty());
    }

    #[test]
    fn ocr_roundtrip() {
        let mut m = BTreeMap::new();
        m.insert(0, "川普 關稅".to_string());
        m.insert(60, "有\t分隔".to_string());
        let back = decode_ocr(&encode_ocr(&m));
        assert_eq!(back[&0], "川普 關稅");
        assert_eq!(back[&60], "有 分隔");
    }

    #[test]
    fn timestamp_from_recorder_name() {
        assert_eq!(
            name_timestamp("#1_Channel 3---2026_10_20---22_00_40---H264  8 bit---188---split001.mp4").as_deref(),
            Some("20261020220040")
        );
        assert_eq!(name_timestamp("第1段.mp4"), None);
    }
}
