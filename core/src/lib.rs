//! 關鍵標題文稿整理：把各段貼上的原始標題稿轉成 YouTube 上片用的文稿格式。
//!
//! 全部是純函式，不碰檔案也不碰 UI，方便用範本做逐字比對測試。

pub mod flash;
pub mod fullwidth;
pub mod import;
pub mod parse;
pub mod render;
pub mod settings;
pub mod timecode;
pub mod video;

pub use render::{render, DocumentInput, Rendered, SegmentInput, Warning};
pub use settings::Settings;
