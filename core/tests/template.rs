//! 用別人做好的 20261014 範本做逐字比對。
//!
//! fixtures/20261014/seg*.txt 是從範本反推回去的輸入（加上「一段」、開場、講者名字和空行），
//! 範本裡的標題都是已經播出、公開在 YouTube 上的標題。

use keyword_title_core::render::{file_name, render, to_crlf, DocumentInput, SegmentInput};
use keyword_title_core::Settings;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/20261014")
}

fn read(name: &str) -> String {
    fs::read_to_string(fixture_dir().join(name)).unwrap()
}

fn render_fixture(settings: Settings) -> String {
    let segments = read("times.txt")
        .lines()
        .enumerate()
        .map(|(i, time)| SegmentInput {
            text: read(&format!("seg{}.txt", i + 1)),
            time: time.to_string(),
            link: String::new(),
        })
        .collect();
    let r = render(&DocumentInput {
        date: "20261014".into(),
        segments,
        full_link: String::new(),
        settings,
    });
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    to_crlf(&r.text)
}

/// 範本檔尾多了一串空行（記事本手動留的），比對時忽略檔尾空白。
fn expected() -> String {
    read("expected.txt").trim_end().to_string()
}

#[test]
fn reproduces_template_exactly_with_header_and_no_labels() {
    let actual = render_fixture(Settings {
        show_header: true,
        halfwidth_timestamps: false,
        label_segments: false,
        number_line: true,
        ..Settings::default()
    });
    assert_eq!(actual.trim_end(), expected());
    assert_eq!(file_name("20261014"), "20261014關鍵標題1-6.txt");
}

#[test]
fn default_output_is_template_from_open_with_segment_labels() {
    // 由範本推出預設格式：保留檔頭，open 換成「第一段」，
    // 之後每個 ======= 的下一行補「第N段」（最後一個 ======= 後面接全集，不補），
    // 並拿掉段標題下方的空行＋「1.」編號行。
    let template = expected();
    let open_at = template.find("open\r\n").unwrap();
    let from_open = &template[open_at + "open\r\n".len()..];
    let names = ["一", "二", "三", "四", "五", "六"];
    let mut want = format!("{}第{}段\r\n", &template[..open_at], names[0]);
    for (i, chunk) in from_open.split("=======\r\n").enumerate() {
        if i > 0 {
            want.push_str("=======\r\n");
            if !chunk.starts_with("全集") {
                want.push_str(&format!("第{}段\r\n", names[i]));
            }
        }
        want.push_str(chunk);
    }
    for n in 1..=6 {
        want = want.replace(&format!("\r\n\r\n{n}.\r\n"), "\r\n");
    }

    let actual = render_fixture(Settings {
        halfwidth_timestamps: false,
        ..Settings::default()
    });
    assert_eq!(actual.trim_end(), want);
}
