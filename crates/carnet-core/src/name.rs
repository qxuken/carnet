//! A note's file name, from its title: the same on every device, so the
//! desktop and the web viewer agree on what a note is called. And what a
//! name or a path from elsewhere (another device, a note's text) must be
//! before it's joined onto the notes folder ([`is_plain_path`]).

/// What some OS forbids in a file's name: a separator (`/`, and `\` on
/// Windows), a drive's `:`, and Windows' wildcards and quotes.
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// A file name for a title: what the OS forbids becomes `-`, trimmed, at
/// most 80 chars, and never a name Windows keeps for a device.
pub fn slug(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| match c {
            c if FORBIDDEN.contains(&c) => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .take(80)
        .collect();
    let mut s = s.trim().trim_matches('.').trim();
    // Never hidden (". .x" would be ".x"): a file the notes folder hides.
    while s.starts_with('.') {
        s = s.trim_start_matches('.').trim_start();
    }
    let s = s.to_string();
    if s.is_empty() || reserved(&s) {
        "Untitled".to_string()
    } else {
        s
    }
}

/// Whether Windows keeps `name` for a device (`CON`, `nul.tar`, `COM7`):
/// a file of that name is the device, in whatever folder.
fn reserved(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or("").trim().to_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit()
            && base.as_bytes()[3] != b'0')
}

/// Whether `part` is one name in a folder on every system Carnet runs on,
/// and nothing more: not empty, not `.` or `..` (nor only dots and spaces,
/// which Windows trims to them), not hidden (`.trash`, `.git`), with
/// nothing [`slug`] takes out (a separator, a drive's `:`) and no control
/// character, and not a name Windows keeps for a device. Anything else
/// joined onto the notes folder could reach outside it.
pub fn is_plain_name(part: &str) -> bool {
    !part.starts_with('.')
        && !part.trim_matches([' ', '.']).is_empty()
        && !part.chars().any(|c| FORBIDDEN.contains(&c) || c.is_control())
        && !reserved(part)
}

/// Whether `path` is a path from the notes folder, `/` between its parts,
/// each an [`is_plain_name`]: a folder, or a picture's path, as the vault
/// holds them.
pub fn is_plain_path(path: &str) -> bool {
    !path.is_empty() && path.split('/').all(is_plain_name)
}

/// The name a note's text asks for: its title's [`slug`] ("Untitled" for
/// none).
pub fn for_text(md: &str) -> String {
    let (title, _) = crate::doc::from_markdown(md);
    slug(title.as_deref().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("  "), "Untitled");
        assert_eq!(slug("con"), "Untitled");
        assert_eq!(slug("COM7"), "Untitled");
        assert_eq!(slug("nul.tar"), "Untitled");
        assert_eq!(slug("COM0"), "COM0");
        assert_eq!(slug("a/b"), "a-b");
        assert_eq!(slug("...x..."), "x");
        assert_eq!(slug(". .x"), "x");
        assert_eq!(for_text("# Plan: A/B\n\nwords\n"), "Plan- A-B");
        assert_eq!(for_text("no title\n"), "Untitled");
    }

    /// What reaches outside the notes folder, or names a device, isn't a
    /// plain name or path; an ordinary one, spaces, accents, dots and all,
    /// is. Whatever `slug` makes is one.
    #[test]
    fn plain_names_and_paths() {
        for ok in ["Plan", "Plan 2", "Trip to Zürich", "日本語のメモ", "v1.2 notes", "a.b.c", "x.", "COM0", "Nulls", "con-tract"] {
            assert!(is_plain_name(ok), "{ok:?}");
        }
        for bad in [
            "", ".", "..", "...", " . ", ". .", ".trash", ".git", "a/b", r"a\b", r"..\x", "C:", "C:x", "a:b", "a\0b", "a\nb", "a\rb",
            "tab\there", "\u{7f}", "\u{85}", "con", "CON", "nul.txt", "Aux .md", "com1", "LPT9.png", "conin$", "a?b", "a*b", "a|b", "<a>",
            "\"q\"",
        ] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
        for ok in ["attachments/a.png", "Work/Plan", "Work/2024/März", "a b/c d.png"] {
            assert!(is_plain_path(ok), "{ok:?}");
        }
        for bad in [
            "",
            "/x",
            "/etc/passwd",
            "x/",
            "a//b",
            "attachments/../../../Library/LaunchAgents/x.plist",
            "attachments/./a.png",
            "attachments/..",
            r"\\host\share\x.png",
            r"\x",
            "C:/Windows/x",
            r"C:\x",
            r"attachments\..\..\x",
            r"a/b\c",
            "a/NUL/b",
            "attachments/a\0.png",
            "attachments/.hidden.png",
        ] {
            assert!(!is_plain_path(bad), "{bad:?}");
        }
        for title in ["../../x", r"C:\Windows", "con", "...", ". . .x", "a/b:c", "\0"] {
            assert!(is_plain_name(&slug(title)), "{title:?} → {:?}", slug(title));
        }
    }
}
