//! Note links: choosing a destination and keeping ordinary Markdown
//! paths pointing at that destination when its stable identity moves.

use std::collections::{BTreeMap, HashMap, HashSet};
use crate::{doc::{self, Doc, Kind}, inline};

pub type Paths = BTreeMap<String, String>;

/// The desktop's deterministic collision suffixes, also used in the web
/// picker. A note with the title "Plan" may really be "Plan 2.md".
pub fn file_paths(notes: Vec<(String, String, String)>) -> Paths {
    type Placement = (String, String, String);
    let mut groups: BTreeMap<(String, String), Vec<Placement>> = BTreeMap::new();
    for (id, folder, name) in notes { groups.entry((folder.to_lowercase(), name.to_lowercase())).or_default().push((id, folder, name)); }
    let plain: HashSet<_> = groups.keys().cloned().collect();
    let mut paths = Paths::new();
    for ((folder_key, name_key), mut notes) in groups {
        notes.sort_by(|a, b| a.0.cmp(&b.0));
        let mut suffixes = (2..).filter(|n| !plain.contains(&(folder_key.clone(), format!("{name_key} {n}"))));
        for (i, (id, folder, name)) in notes.into_iter().enumerate() {
            let stem = if i == 0 { name } else { format!("{name} {}", suffixes.next().unwrap()) };
            paths.insert(id, if folder.is_empty() { format!("{stem}.md") } else { format!("{folder}/{stem}.md") });
        }
    }
    paths
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub fragment: String,
}

pub fn sections(doc: &Doc) -> Vec<Section> {
    let mut used: HashSet<String> = doc.blocks().iter().filter_map(|b| match &b.kind { Kind::Text(t) => doc::anchor_id(t), _ => None }).collect();
    doc.blocks().iter().filter_map(|b| match &b.kind {
        Kind::Text(t) => doc::anchor_id(t).map(|id| Section { name: format!("#{id}"), fragment: format!("#{}", encode(&id)) }),
        Kind::Heading { text, .. } => Some(Section { name: inline::plain(text), fragment: format!("#{}", encode(&doc::heading_id(text, &mut used))) }),
        _ => None,
    }).collect()
}

/// Titles first; the path is searchable too and disambiguates names.
pub fn search(notes: &[Note], query: &str) -> Vec<Note> {
    let query = query.trim().to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    let mut found: Vec<_> = notes.iter().filter(|n| words.iter().all(|w| format!("{} {}", n.title, n.path).to_lowercase().contains(w))).cloned().collect();
    found.sort_by_key(|n| (!n.title.to_lowercase().starts_with(&query), n.title.to_lowercase(), n.path.to_lowercase()));
    found
}

pub fn folder(path: &str) -> &str { path.rsplit_once('/').map_or("", |(f, _)| f) }

/// Pasting an address into the note search can use the address tab.
pub fn typed_address(text: &str) -> bool {
    if text.contains("://") || text.starts_with("mailto:") || text.contains('@') { return true; }
    if text.chars().any(char::is_whitespace) { return false; }
    let host = text.split('/').next().unwrap_or("");
    host.rsplit_once('.').is_some_and(|(name, suffix)| !name.is_empty() && !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_alphabetic()))
}

/// A destination relative to its source, encoded for a Markdown URL.
pub fn href(source: &str, target: &str, fragment: Option<&str>) -> String {
    if source == target && let Some(fragment) = fragment { return fragment.to_string(); }
    let from: Vec<_> = folder(source).split('/').filter(|p| !p.is_empty()).collect();
    let to: Vec<_> = target.split('/').collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let path = std::iter::repeat_n("..", from.len() - common).chain(to[common..].iter().copied()).collect::<Vec<_>>().join("/");
    format!("{}{}", encode(&path), fragment.unwrap_or(""))
}

pub fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) { out.push(b as char); }
        else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

/// Only destinations whose stable identity changed path. No scan of
/// document contents is needed when nothing was renamed or moved.
pub fn moves(before: &Paths, after: &Paths) -> HashMap<String, String> {
    before.iter().filter_map(|(id, old)| after.get(id).filter(|new| *new != old).map(|new| (old.clone(), new.clone()))).collect()
}

/// Changes URL bytes alone, preserving labels, titles, fragments, line
/// endings and examples in code. The paths are from the notes root.
pub fn rewrite(md: &str, source_before: &str, source_after: &str, moved: &HashMap<String, String>) -> Option<String> {
    let mut edits = Vec::new();
    let mut offset = 0;
    let mut fence: Option<(u8, usize)> = None;
    let mut front = false;
    let has_front = md.lines().next().is_some_and(|l| l.trim() == "---") && md.lines().skip(1).any(|l| matches!(l.trim(), "---" | "..."));
    for (line_no, line) in md.split_inclusive('\n').enumerate() {
        let trimmed = line.trim();
        if line_no == 0 && has_front { front = true; offset += line.len(); continue; }
        if front {
            if matches!(trimmed, "---" | "...") { front = false; }
            offset += line.len(); continue;
        }
        let bytes = trimmed.as_bytes();
        let run = bytes.first().filter(|c| matches!(c, b'`' | b'~')).map(|ch| (*ch, bytes.iter().take_while(|b| *b == ch).count()));
        if let Some((ch, n)) = run.filter(|(_, n)| *n >= 3) {
            match fence {
                None => fence = Some((ch, n)),
                Some((open, len)) if ch == open && n >= len && bytes[n..].iter().all(u8::is_ascii_whitespace) => fence = None,
                _ => {}
            }
            offset += line.len(); continue;
        }
        if fence.is_none() {
            for p in inline::pairs(line).into_iter().filter(|p| p.kind == inline::Kind::Link && line.get(p.close.0..p.close.0 + 2) == Some("](")) {
                let start = p.close.0 + 2;
                let inner = &line[start..p.close.1 - 1];
                let (from, to) = if let Some(rest) = inner.strip_prefix('<') {
                    let Some(end) = rest.find('>') else { continue };
                    (start + 1, start + 1 + end)
                } else {
                    (start, start + inner.find(char::is_whitespace).unwrap_or(inner.len()))
                };
                let url = &line[from..to];
                let Some((target, fragment)) = doc::note_link(folder(source_before), url) else { continue };
                let target = moved.get(&target).unwrap_or(&target);
                let new = href(source_after, target, fragment.as_deref());
                // Preserve the original spelling if the relative destination
                // did not change, including ./ and existing URL escapes.
                if doc::note_link(folder(source_after), url).as_ref().map(|(p, _)| p) != Some(target) {
                    edits.push((offset + from..offset + to, new));
                }
            }
        }
        offset += line.len();
    }
    if edits.is_empty() { return None; }
    let mut out = md.to_string();
    edits.sort_by_key(|(r, _)| r.start);
    for (range, new) in edits.into_iter().rev() { out.replace_range(range, &new); }
    Some(out)
}

/// The same repair in an editor's block model: block and cell identities
/// stay in place, so a link change does not replace the whole document.
pub fn rewrite_doc(doc: &mut Doc, before: &str, after: &str, moved: &HashMap<String, String>) -> bool {
    let mut changed = false;
    for i in 0..doc.len() {
        match &mut doc.block_mut(i).kind {
            Kind::Raw(_) => {}
            Kind::Table(t) => for cell in t.rows.iter_mut().flat_map(|r| &mut r.cells) {
                if let Some(new) = rewrite(cell, before, after, moved) { *cell = new; changed = true; }
            },
            _ => if let Some(text) = doc.block_mut(i).text_mut() && let Some(new) = rewrite(text, before, after, moved) { *text = new; changed = true; },
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_follow_both_ends_without_rewriting_examples_or_labels() {
        let moved = HashMap::from([("docs/Old name.md".into(), "Archive/New name.md".into())]);
        let md = "[**Label**](<docs/Old%20name.md#decision> \"keep title\")\r\n`[example](docs/Old%20name.md)`\r\n```md\r\n[example](docs/Old%20name.md)\r\n```\r\n[web](https://x/Old.md)\r\n";
        let out = rewrite(md, "Index.md", "Work/Index.md", &moved).unwrap();
        assert!(out.starts_with("[**Label**](<../Archive/New%20name.md#decision> \"keep title\")\r\n"), "{out}");
        assert!(out.contains("`[example](docs/Old%20name.md)`"));
        assert!(out.contains("```md\r\n[example](docs/Old%20name.md)\r\n```"));
        assert!(out.contains("https://x/Old.md"));
        assert_eq!(rewrite(&out, "Work/Index.md", "Work/Index.md", &moved), None);
        assert_eq!(href("docs/Index.md", "docs/A # plan.md", Some("#decision")), "A%20%23%20plan.md#decision");
        assert!(rewrite("---\n\n[link](docs/Old%20name.md)\n", "Index.md", "Index.md", &moved).unwrap().contains("Archive/New%20name.md"));
    }

    #[test]
    fn sections_and_search_are_shared_by_both_pickers() {
        let (_, doc) = doc::from_markdown("## Decision\n\n## Decision\n\n<a id=\"explicit\"></a>");
        assert_eq!(sections(&doc).iter().map(|s| s.fragment.as_str()).collect::<Vec<_>>(), ["#decision", "#decision-1", "#explicit"]);
        let notes = vec![Note { id: "1".into(), title: "Plan".into(), path: "Work/Plan.md".into() }, Note { id: "2".into(), title: "Plan".into(), path: "Home/Plan.md".into() }];
        assert_eq!(search(&notes, "plan work").iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["1"]);
        let paths = file_paths(vec![("b".into(), "Work".into(), "Plan".into()), ("a".into(), "Work".into(), "Plan".into()), ("c".into(), "Work".into(), "Plan 2".into())]);
        assert_eq!(paths["a"], "Work/Plan.md");
        assert_eq!(paths["b"], "Work/Plan 3.md");
        assert_eq!(paths["c"], "Work/Plan 2.md");
    }


}
