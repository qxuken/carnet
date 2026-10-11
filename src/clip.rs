//! The clipboard beyond text, which kui's own paste and copy carry: a
//! picture, or files copied in a file manager. Tests never touch the
//! system clipboard.

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

/// Puts a picture on the clipboard.
#[cfg(not(test))]
pub fn write_image(w: u32, h: u32, rgba: Vec<u8>) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_image(arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: rgba.into(),
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
pub fn write_image(_w: u32, _h: u32, _rgba: Vec<u8>) -> Result<(), String> {
    Ok(())
}
