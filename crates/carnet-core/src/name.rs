//! A note's file name, from its title: the same on every device, so the
//! desktop and the web viewer agree on what a note is called.

/// A file name for a title: what the OS forbids becomes `-`, trimmed, at
/// most 80 chars, and never a name Windows keeps for a device.
pub fn slug(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .take(80)
        .collect();
    let s = s.trim().trim_matches('.').trim().to_string();
    let base = s.split('.').next().unwrap_or("").trim().to_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit()
            && base.as_bytes()[3] != b'0');
    if s.is_empty() || reserved {
        "Untitled".to_string()
    } else {
        s
    }
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
        assert_eq!(for_text("# Plan: A/B\n\nwords\n"), "Plan- A-B");
        assert_eq!(for_text("no title\n"), "Untitled");
    }
}
