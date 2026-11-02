//! 封面 / 正文图片落盘：按内容哈希命名，天然去重。
//! EPUB 从 zip 里取，MOBI 从 PalmDB 记录里取，取到之后都走这一个入口。

use std::path::Path;

use sha2::{Digest, Sha256};

/// `ext` 传扩展名（不带点）；传空串时交给 `sniff_ext` 猜。
/// 返回站内路径 `/media/{book_id}/{hash}.{ext}`，失败返回 None（图片丢了不影响正文）。
pub fn save(dir: &Path, book_id: &str, ext: &str, bytes: &[u8]) -> Option<String> {
    let ext = if ext.is_empty() {
        sniff_ext(bytes).to_string()
    } else {
        ext.to_string()
    };
    let ext = if ext.len() <= 5 { ext.to_lowercase() } else { "bin".to_string() };
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let name = format!("{}.{}", hex8(&digest[..8]), ext);
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(dir.join(&name), bytes).ok()?;
    Some(format!("/media/{book_id}/{name}"))
}

/// 按文件头猜图片格式。MOBI 的记录里没有扩展名，只能看魔数。
pub fn sniff_ext(b: &[u8]) -> &'static str {
    if b.len() >= 3 && b[0] == 0xff && b[1] == 0xd8 && b[2] == 0xff {
        "jpg"
    } else if b.len() >= 8 && &b[..8] == b"\x89PNG\r\n\x1a\n" {
        "png"
    } else if b.len() >= 6 && (&b[..6] == b"GIF87a" || &b[..6] == b"GIF89a") {
        "gif"
    } else if b.len() >= 2 && &b[..2] == b"BM" {
        "bmp"
    } else {
        "bin"
    }
}

pub fn hex8(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
