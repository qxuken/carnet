//! What a paste brings beyond text, which kui's own paste carries: a
//! picture, or files copied in a file manager. A copy, text or picture,
//! goes out through kui. Tests never touch the system clipboard.

use std::path::PathBuf;

/// What a paste brings that is not text.
#[cfg_attr(test, allow(dead_code))]
pub enum Media {
    Image { w: u32, h: u32, rgba: Vec<u8> },
    Files(Vec<PathBuf>),
}

/// The clipboard's picture or files — `None` when it holds text, which
/// wins: Word and Excel put a picture of what was copied beside its text,
/// and a paste of a paragraph must not be a screenshot of it.
#[cfg(not(test))]
pub fn read_media() -> Option<Media> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if cb.get_text().is_ok_and(|t| !t.trim().is_empty()) {
        return None;
    }
    if let Ok(img) = cb.get_image() {
        return Some(Media::Image {
            w: img.width as u32,
            h: img.height as u32,
            rgba: img.bytes.into_owned(),
        });
    }
    match cb.get().file_list() {
        Ok(files) if !files.is_empty() => Some(Media::Files(files)),
        _ => None,
    }
}

#[cfg(test)]
pub fn read_media() -> Option<Media> {
    None
}
