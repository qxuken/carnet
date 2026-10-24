//! Markdown files dropped on the page come in as notes, as the desktop
//! brings them in (src/store.rs `import_note`). A browser hands the page
//! the files dropped and nothing beside them, so the pictures and files a
//! note links to by a path from where it was come along only when they
//! were dropped with it: each is found by its name, put in the attachments
//! folder as any file put in here is, and the note's links follow it. A
//! link to one not dropped stays as written, as one to a file not there
//! does on the desktop.

use crate::relink::{decode, is_relative};

/// Where each link's target is in `md`: `](target)`, `](<target>)` (a title
/// after it left out) and an HTML `src="…"`, by their byte ranges, in
/// order. The desktop's `link_targets`.
pub fn link_targets(md: &str) -> Vec<std::ops::Range<usize>> {
    let b = md.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = md[i..].find("](") {
        let start = i + at + 2;
        if b.get(start) == Some(&b'<') {
            let Some(len) = md[start + 1..].find(['>', '\n']) else { break };
            let end = start + 1 + len;
            if b[end] == b'>' && len > 0 {
                out.push(start + 1..end);
            }
            i = end;
            continue;
        }
        // Up to the `)` that closes it, parentheses inside kept in pairs.
        let mut depth = 0;
        let mut end = start;
        while end < b.len() {
            match b[end] {
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b'\n' => break,
                _ => {}
            }
            end += 1;
        }
        if b.get(end) == Some(&b')') {
            // A title after it: `(src "title")`.
            let t = &md[start..end];
            let t = t.find(" \"").or_else(|| t.find(" '")).map_or(t, |s| &t[..s]).trim_end();
            if !t.is_empty() {
                out.push(start..start + t.len());
            }
        }
        i = end.max(start);
    }
    for q in ["src=\"", "src='"] {
        let close = q.as_bytes()[4] as char;
        let mut i = 0;
        while let Some(at) = md[i..].find(q) {
            let start = i + at + q.len();
            let Some(len) = md[start..].find([close, '\n', '>']) else { break };
            if md[start + len..].starts_with(close) && len > 0 {
                out.push(start..start + len);
            }
            i = start + len;
        }
    }
    out.sort_by_key(|r| r.start);
    out
}

/// Where each Obsidian link's and embed's target is: `[[target]]`,
/// `![[target|300]]`, on one line, spaces around it left out. The
/// desktop's `wiki_targets`.
pub fn wiki_targets(md: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = md[i..].find("[[") {
        let start = i + at + 2;
        let Some(len) = md[start..].find(['|', ']', '[', '\n']) else { break };
        let end = start + len;
        let line = &md[end..md[end..].find('\n').map_or(md.len(), |n| end + n)];
        if (line.starts_with('|') && line.contains("]]")) || line.starts_with("]]") {
            let t = &md[start..end];
            let lead = t.len() - t.trim_start().len();
            if !t.trim().is_empty() {
                out.push(start + lead..start + lead + t.trim().len());
            }
        }
        i = end;
    }
    out
}

/// The file a target names, by its name alone, case aside: what a file
/// dropped beside the note is matched by. None for a web address, a full
/// path, a place in the note, or another note (a `.md`), which comes in
/// on its own or not at all.
fn named(target: &str) -> Option<String> {
    if !is_relative(target) || target.starts_with('#') {
        return None;
    }
    let path = decode(target);
    let name = path.rsplit(['/', '\\']).next()?.trim();
    let lower = name.to_lowercase();
    (!name.is_empty() && !lower.ends_with(".md") && !lower.ends_with(".markdown") && name != "." && name != "..").then_some(lower)
}

/// The names of the files `md` links to or embeds by a path, lowercase,
/// each once: of the files dropped with it, those to bring.
pub fn linked_names(md: &str) -> Vec<String> {
    let mut out: Vec<String> = link_targets(md).into_iter().chain(wiki_targets(md)).filter_map(|r| named(&md[r])).collect();
    out.sort();
    out.dedup();
    out
}

/// A path as a link's target: a space, a parenthesis, an angle bracket
/// or a quote written as its `%` escape, so the link reads the same with
/// or without `<…>` round it. The desktop's `encode_src`.
fn encode_src(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '"' => out.push_str("%22"),
            '\'' => out.push_str("%27"),
            c => out.push(c),
        }
    }
    out
}

/// `md`, a note coming into `folder`, its links to the files `brought`
/// (each its name as dropped, and the path from the notes folder it was
/// put at) rewritten to find them there: a link by its path from the
/// note, an Obsidian embed by its path from the notes folder. A link to
/// any other file stays as written.
pub fn brought(md: &str, folder: &str, brought: &[(String, String)]) -> String {
    let at = |t: &str| -> Option<&str> {
        let name = named(t)?;
        brought.iter().find(|(n, _)| n.to_lowercase() == name).map(|(_, p)| p.as_str())
    };
    let depth = folder.split('/').filter(|p| !p.is_empty()).count();
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for r in link_targets(md) {
        if let Some(p) = at(&md[r.clone()]) {
            edits.push((r, encode_src(&format!("{}{p}", "../".repeat(depth)))));
        }
    }
    for r in wiki_targets(md) {
        if let Some(p) = at(&md[r.clone()]).filter(|p| !p.contains(['[', ']', '|', '#'])) {
            edits.push((r, p.to_string()));
        }
    }
    if edits.is_empty() {
        return md.to_string();
    }
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(md.len());
    let mut last = 0;
    for (r, new) in edits {
        if r.start < last {
            continue;
        }
        out.push_str(&md[last..r.start]);
        out.push_str(&new);
        last = r.end;
    }
    out.push_str(&md[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A note's links and embeds are found by their targets, a title or a
    /// width after one left out; a web address, a place in the note and
    /// another note are no file to bring.
    #[test]
    fn the_files_a_note_names_are_found_by_name() {
        let md = "# Trip\n\n![The sea](img/Sea%20View.png \"sea\")\n\nSee [the plan](<../docs/plan (v2).pdf>) and [x](https://e.com/a.png), \
                  [up](#top), [other](Other.md).\n\n![[hill.jpg|300]] and <img src=\"pics/a.gif\">\n";
        assert_eq!(linked_names(md), ["a.gif", "hill.jpg", "plan (v2).pdf", "sea view.png"]);
        assert!(linked_names("# Plain\n\nwords\n").is_empty());
    }

    /// The files dropped with a note are linked where they were put, from
    /// the folder it goes into; the links to files not dropped stay as
    /// they were, and nothing else in it changes.
    #[test]
    fn links_follow_the_files_brought_with_a_note() {
        let md = "# Trip\n\n![The sea](img/Sea%20View.png \"sea\")\n\nSee [the plan](<../docs/plan (v2).pdf>), [gone](missing.pdf) \
                  and ![[hill.jpg|300]].\n";
        let files = [
            ("Sea View.png".to_string(), "attachments/Sea-View.png".to_string()),
            ("plan (v2).pdf".to_string(), "attachments/plan-(v2).pdf".to_string()),
            ("HILL.jpg".to_string(), "attachments/HILL.jpg".to_string()),
        ];
        let at_top = brought(md, "", &files);
        assert_eq!(
            at_top,
            "# Trip\n\n![The sea](attachments/Sea-View.png \"sea\")\n\nSee [the plan](<attachments/plan-%28v2%29.pdf>), [gone](missing.pdf) \
             and ![[attachments/HILL.jpg|300]].\n"
        );
        let deeper = brought(md, "Work/Trips", &files);
        assert!(deeper.contains("![The sea](../../attachments/Sea-View.png \"sea\")"), "{deeper}");
        assert!(deeper.contains("![[attachments/HILL.jpg|300]]"), "an embed by its path from the notes folder: {deeper}");
        assert_eq!(brought(md, "", &[]), md);
    }
}
