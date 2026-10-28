//! A note moved to another folder still shows its pictures: the paths
//! written in it are rewritten from where it now is, as the desktop's move
//! rewrites them (src/store.rs `move_note`), and the note's new text goes
//! as this device's edit, which the desktop that has the files takes as it
//! is (`place_note`). The browser has no files to move, so every picture
//! stays where it is: a folder moved from here is its notes moved one by
//! one, each rewritten so.

use carnet_core::doc::{self, Kind};

/// `md`, a note's text written in the folder `from`, as it reads from
/// `to`: each picture written as a path (`![](…)`) still the same file,
/// and one an Obsidian embed names (`![[…]]`) that wouldn't be found from
/// `to` named by its path from the notes folder. `files` are the pictures
/// the vault names, what an embed is looked for among. `None` when no path
/// changes.
pub fn relinked(md: &str, from: &str, to: &str, files: &doc::Files) -> Option<String> {
    if from == to {
        return None;
    }
    let (title, mut d) = doc::from_markdown(md);
    let moved = |src: &str, wiki: bool| -> Option<String> {
        if wiki {
            let was = doc::find_picture(from, src, true, files);
            return was.filter(|w| doc::find_picture(to, src, true, files).as_ref() != Some(w));
        }
        is_relative(src).then(|| moved_src(from, to, src))
    };
    let mut changed = false;
    for b in 0..d.len() {
        let new = match &d.blocks()[b].kind {
            Kind::Image { src, wiki, .. } => moved(src, *wiki),
            // A table's pictures among its cells' words, the same way.
            Kind::Table(t) => {
                let mut cells = t.clone();
                let mut any = false;
                for cell in cells.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
                    if let Some(new) = doc::relink_cell(cell, |p| moved(&p.src, p.wiki)) {
                        *cell = new;
                        any = true;
                    }
                }
                if any {
                    d.block_mut(b).kind = Kind::Table(cells);
                    changed = true;
                }
                None
            }
            _ => None,
        };
        if let Some(new) = new
            && let Kind::Image { src, .. } = &mut d.block_mut(b).kind
            && *src != new
        {
            *src = new;
            changed = true;
        }
    }
    changed.then(|| doc::to_markdown(title.as_deref(), &d))
}

/// Whether `src` is a path from the note's folder: not a web address, the
/// picture itself (`data:`), or a full path (`/…`, `\\…`, `C:…`).
pub(crate) fn is_relative(src: &str) -> bool {
    let b = src.as_bytes();
    let drive = b.len() > 1 && b[1] == b':' && b[0].is_ascii_alphabetic();
    !(src.contains("://") || src.starts_with("data:") || src.starts_with(['/', '\\']) || drive)
}

/// `src`, a path from the folder `from`, as a path from `to` to the same
/// file, `..` worked out as written. One out of the notes folder stays out
/// of it, by as many steps more as `to` is deep. As the desktop writes it:
/// `%20` and the like as the characters they stand for.
fn moved_src(from: &str, to: &str, src: &str) -> String {
    let mut at: Vec<String> = from.split('/').filter(|p| !p.is_empty()).map(str::to_string).collect();
    // How far above the notes folder it climbs.
    let mut above = 0;
    for part in decode(src).split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if at.pop().is_none() {
                    above += 1;
                }
            }
            p => at.push(p.to_string()),
        }
    }
    let here: Vec<&str> = to.split('/').filter(|p| !p.is_empty()).collect();
    let common = if above > 0 { 0 } else { here.iter().zip(&at).take_while(|(a, b)| a.to_lowercase() == b.to_lowercase()).count() };
    let mut out = vec!["..".to_string(); here.len() - common + above];
    out.extend(at[common..].iter().cloned());
    out.join("/")
}

/// `%20` and friends back to their characters, as a link another editor
/// wrote may have them (the desktop's `decode_src`).
pub(crate) fn decode(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |x: u8| (x as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_worked_out_from_the_new_folder() {
        assert_eq!(moved_src("", "Work", "attachments/a.png"), "../attachments/a.png");
        assert_eq!(moved_src("Work", "", "../attachments/a%20b.png"), "attachments/a b.png");
        assert_eq!(moved_src("Home", "Projects/Garden", "../attachments/p.jpg"), "../../attachments/p.jpg");
        assert_eq!(moved_src("Home", "Home/Sub", "hill.jpg"), "../hill.jpg", "a picture beside it stays there");
        assert_eq!(moved_src("Work", "Home", "./../attachments/./a.png"), "../attachments/a.png");
        assert_eq!(moved_src("", "Work", "../Pictures/sea.png"), "../../Pictures/sea.png", "out of the notes folder, still");
        assert_eq!(moved_src("Work/Q4", "work", "../../attachments/a.png"), "../attachments/a.png", "case aside");
        assert!(!is_relative("https://e.com/a.png") && !is_relative("/Users/me/a.png") && !is_relative("C:\\a.png") && !is_relative("data:image/png;base64,AA"));
        assert!(is_relative("attachments/a.png") && is_relative("../a.png"));
    }

    #[test]
    fn a_notes_pictures_follow_it_and_nothing_else_changes() {
        let files = doc::Files::new(["attachments/2026/photo.png", "Home/hill.jpg"]);
        let md = "# Shop\n\n- [ ] milk\n\n![](../attachments/p.png)\n\n![The hill](hill.jpg)\n\n![[photo.png|200]]\n\n![](https://example.com/a.png)\n\n| a | b |\n| --- | --- |\n| ![](../attachments/c.png) | x |\n";
        let moved = relinked(md, "Home", "", &files).expect("rewritten");
        assert!(moved.contains("![](attachments/p.png)"), "{moved}");
        assert!(moved.contains("![The hill](Home/hill.jpg)"), "{moved}");
        assert!(moved.contains("![[photo.png|200]]"), "found from anywhere: as it was: {moved}");
        assert!(moved.contains("![](https://example.com/a.png)"), "{moved}");
        assert!(moved.contains("| ![](attachments/c.png) | x |"), "{moved}");
        assert!(moved.starts_with("# Shop\n\n- [ ] milk\n"), "{moved}");
        // Back again: as it was.
        assert_eq!(relinked(&moved, "", "Home", &files).as_deref(), Some(md));
        // An embed found beside it, where from elsewhere another of its
        // name would be: named by its path once away.
        let files = doc::Files::new(["attachments/hill.jpg", "Home/hill.jpg"]);
        let beside = "# Hill\n\n![[hill.jpg]]\n";
        assert_eq!(relinked(beside, "Home", "", &files).as_deref(), Some("# Hill\n\n![[Home/hill.jpg]]\n"));
        // No picture, or none whose path changes: nothing to write.
        assert_eq!(relinked("# Plan\n\nwords\n", "", "Work", &files), None);
        assert_eq!(relinked("# Plan\n\n![](https://e.com/a.png)\n", "", "Work", &files), None);
        assert_eq!(relinked(md, "Home", "Home", &files), None);
    }
}
