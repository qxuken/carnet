//! Searching the words the page draws, with source positions for the jump.
//! The cache belongs to an editor, so each note window has its own find bar.

use super::*;
use crate::icons::{Glyph, icon};

pub(super) const BAR_H: f32 = 64.0;

struct Source {
    note: u64,
    title: String,
    query: String,
    blocks: Vec<Rc<Block>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    Title(usize, usize),
    Body(Caret),
}

#[derive(Default)]
pub(super) struct Find {
    pub open: bool,
    pub focused: bool,
    pub query: String,
    pub hits: Vec<Hit>,
    pub active: Option<usize>,
    source: Option<Source>,
    pub title_range: Option<(usize, usize)>,
}

/// Case-insensitive literal matches, in original UTF-8 bytes. Lowercasing
/// can change byte lengths (İ, for example), so offsets into the lowered
/// string must never be used as offsets into the note.
fn ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let needle: String = query.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut folded = String::new();
    let mut boundaries = Vec::new();
    for (i, ch) in text.char_indices() {
        boundaries.push((folded.len(), i));
        folded.extend(ch.to_lowercase());
    }
    boundaries.push((folded.len(), text.len()));
    let original = |i| boundaries.binary_search_by_key(&i, |b| b.0).ok().map(|k| boundaries[k].1);
    folded.match_indices(&needle).filter_map(|(a, s)| original(a).zip(original(a + s.len()))).collect()
}

fn body_hits<'a>(doc: &'a Doc, query: &str) -> impl Iterator<Item = Hit> + 'a {
    // Own the query so the returned iterator only borrows the document.
    let query = query.to_string();
    let slots = if query.is_empty() { Vec::new() } else { page::slots(doc) };
    slots.into_iter().flat_map(move |slot| {
        let visible = page::drawn(doc, slot);
        let words = visible.text(page::text(doc, slot));
        ranges(&words, &query).into_iter().filter_map(move |(a, b)| {
            let spans = visible.spans(a, b);
            Some(Hit::Body(Caret {
                anchor: Pos { slot, byte: spans.first()?.0 },
                head: Pos { slot, byte: spans.last()?.1 },
            }))
        })
    })
}

pub(super) fn matches(title: &str, doc: &Doc, query: &str) -> bool {
    !ranges(title, query).is_empty() || body_hits(doc, query).next().is_some()
}

/// Keep the first match near the beginning of a preview, including when
/// the preceding context would otherwise push it beyond the visible card.
/// `budget` is the measured width of one or two lines, not a character count.
pub(super) fn excerpt(text: &str, query: &str, budget: f32, mut measure: impl FnMut(&str) -> f32) -> String {
    let Some(&(a, b)) = ranges(text, query).first() else { return text.to_string() };
    let mut start = a;
    for (i, _) in text[..a].char_indices().rev() {
        if measure(&text[i..a]) > budget * 0.15 { break; }
        start = i;
    }
    // Avoid beginning a context fragment halfway through a word.
    if start > 0 && !text[..start].ends_with(char::is_whitespace)
        && let Some(gap) = text[start..a].find(char::is_whitespace)
    {
        start += gap;
    }
    let prefix = if start > 0 { "…" } else { "" };
    let mut end = b;
    for (i, ch) in text[b..].char_indices() {
        let next = b + i + ch.len_utf8();
        if measure(&format!("{prefix}{}…", &text[start..next])) > budget { break; }
        end = next;
    }
    format!("{prefix}{}{}", &text[start..end], if end < text.len() { "…" } else { "" })
}

pub(super) fn snippet(doc: &Doc, query: &str, width: f32, measure: impl FnMut(&str) -> f32) -> Option<String> {
    let Hit::Body(c) = body_hits(doc, query).next()? else { return None };
    let v = page::drawn(doc, c.head.slot);
    let text = v.text(page::text(doc, c.head.slot)).replace(['\n', '\r', '\t'], " ");
    Some(excerpt(&text, query, width * 1.8, measure))
}

/// Search has its own colour roles; the current result also has an
/// underline, so it remains distinct without relying on hue alone.
pub(super) fn mark<'a>(span: kui_native::Span<'a>, l: &Look, current: bool) -> kui_native::Span<'a> {
    let span = span.bg(if current { l.c.search_current } else { l.c.search_match }).bg_radius(3.0).bg_pad(3.0);
    if current { span.color(l.c.search_on_current).underline() } else { span.color(l.t.fg) }
}

pub(super) fn spans<'a>(text: &'a str, hits: &[(usize, usize, bool)], l: &Look) -> Vec<kui_native::Span<'a>> {
    let mut out = Vec::new();
    let mut at = 0;
    for &(a, b, current) in hits {
        if at < a { out.push(kui_native::Span::new(&text[at..a])); }
        out.push(mark(kui_native::Span::new(&text[a..b]), l, current));
        at = b;
    }
    if at < text.len() { out.push(kui_native::Span::new(&text[at..])); }
    out
}

pub(super) fn result_spans<'a>(text: &'a str, query: &str, l: &Look) -> Vec<kui_native::Span<'a>> {
    let hits: Vec<_> = ranges(text, query).into_iter().map(|(a, b)| (a, b, false)).collect();
    let mut out = spans(text, &hits, l);
    for span in &mut out { if span.bg.is_some() { span.bold = true; } }
    out
}


impl Carnet {
    pub(super) fn refresh_find(&mut self) {
        if !self.find.open {
            return;
        }
        let note = &self.store.notes[self.current];
        let title = if let Some(p) = &self.peek { &p.title } else if let Some(v) = &self.versions { &v.title } else { &note.title };
        let blocks = self.doc().blocks();
        if self.find.source.as_ref().is_some_and(|s| {
            s.note == note.id && &s.title == title && s.query == self.find.query && s.blocks.len() == blocks.len()
                && s.blocks.iter().zip(blocks).all(|(a, b)| Rc::ptr_eq(a, b))
        }) {
            return;
        }
        let query = self.find.query.trim();
        let previous = self.find.active.and_then(|i| self.find.hits.get(i)).copied();
        let hits = ranges(title, query).into_iter().map(|(a, b)| Hit::Title(a, b)).chain(body_hits(self.doc(), query)).collect::<Vec<_>>();
        let source = Source { note: note.id, title: title.clone(), query: self.find.query.clone(), blocks: blocks.to_vec() };
        self.find.active = previous.and_then(|h| hits.iter().position(|&v| v == h)).or_else(|| (!hits.is_empty()).then_some(0));
        self.find.hits = hits;
        self.find.source = Some(source);
    }

    pub(super) fn start_find(&mut self) {
        self.settings_open = false;
        self.list_view = false;
        self.find.open = true;
        self.focus_page = false;
        self.refresh_find();
        self.focus(Field::Find);
    }

    pub(super) fn change_find(&mut self, text: String) {
        self.find.query = text;
        self.refresh_find();
        self.find.active = (!self.find.hits.is_empty()).then_some(0);
        self.show_find_hit(false);
    }

    pub(super) fn step_find(&mut self, back: bool) {
        self.refresh_find();
        let len = self.find.hits.len();
        if len == 0 {
            return;
        }
        self.find.active = Some(match self.find.active {
            Some(i) if back => (i + len - 1) % len,
            Some(i) => (i + 1) % len,
            None if back => len - 1,
            None => 0,
        });
        self.show_find_hit(false);
    }

    fn show_find_hit(&mut self, focus: bool) {
        let Some(hit) = self.find.active.and_then(|i| self.find.hits.get(i)).copied() else { return };
        self.typing = false;
        match hit {
            Hit::Body(c) => {
                self.find.title_range = None;
                self.move_to(c);
                if focus {
                    self.focus_req = None;
                    self.focus_page = true;
                    self.editing = true;
                }
            }
            Hit::Title(a, b) => {
                self.find.title_range = Some((a, b));
                if focus && !self.reading() {
                    self.focus(Field::Title);
                }
            }
        }
        self.say = Some(format!("{} of {} matches", self.find.active.unwrap() + 1, self.find.hits.len()));
    }

    pub(super) fn close_find(&mut self) {
        self.find.open = false;
        self.find.title_range = None;
        if self.note_window.is_none() && !self.query.is_empty() {
            self.query.clear();
            self.reorder();
        }
        self.focus_req = None;
        self.focus_page = true;
    }

    /// Every way of opening a library result uses this after the note has
    /// been refreshed from disk: its old byte offsets may no longer exist.
    pub(super) fn open_result(&mut self, note: usize) {
        self.open(note);
        if self.query.trim().is_empty() {
            return;
        }
        self.find.open = true;
        self.find.query = self.query.trim().to_string();
        self.refresh_find();
        self.find.active = (!self.find.hits.is_empty()).then_some(0);
        self.show_find_hit(true);
    }

    pub(super) fn find_bar(&mut self, ui: &mut Ui<'_>, l: &Look) {
        if !self.find.open { return; }
        self.refresh_find();
        let t = l.t;
        let vw = ui.viewport().w;
        let width = if self.note_window.is_some() { vw } else { vw - self.side_w(vw < self.library_least() + PAGE_MIN) };
        let compact = width < 480.0;
        ui.with_keyed("find-bar", NodeSpec::row().grow_width().height(BAR_H).pad_xy(12.0, 8.0).main_align(Align::Center), |ui| {
            ui.with_keyed("find-panel", NodeSpec::row().grow_width().max_width(660.0).height(48.0)
                .pad_xy(10.0, 6.0).gap(if compact { 6.0 } else { 8.0 }).cross_align(Align::Center)
                .bg(l.c.sunken).radius(radius::CONTROL), |ui| {
                ui.text(if compact { "Note" } else { "This note" }, TextStyle::new(Type::SMALL.size).color(t.fg).nowrap());
                ui.with_keyed("find-input", NodeSpec::row().grow_width().min_width(if compact { 40.0 } else { 50.0 }).height(34.0)
                    .pad_xy(8.0, 4.0).gap(6.0).cross_align(Align::Center).bg(l.c.page).radius(5.0)
                    .border(if self.focus_req == Some(Field::Find) || self.find.focused { 1.5 } else { 0.0 }, t.focus_ring), |ui| {
                    icon(ui, Glyph::Search, t.muted, 14.0, false);
                    let k = ui.text_edit_value("find", &self.find.query, &EditOptions {
                        style: TextStyle::new(Type::UI.size).color(t.fg),
                        placeholder: Some("Find in note".into()),
                        keep_tab: true,
                        ..Default::default()
                    }, Field::Find.on(NodeSpec::row().grow_width().min_width(0.0).label("Find in note")));
                    self.fields.insert(k, Field::Find);
                    let requested = self.focus_req == Some(Field::Find);
                    self.apply_focus(ui, k, Field::Find);
                    if requested { ui.core().edit.set_selection(k, (0, 0), (0, usize::MAX)); }
                });
                let empty = self.find.query.trim().is_empty();
                let missing = !empty && self.find.hits.is_empty();
                let count = if empty { if compact { String::new() } else { "Type to find".to_string() } }
                    else if missing { "No matches".to_string() }
                    else if compact { format!("{}/{}", self.find.active.map_or(0, |i| i + 1), self.find.hits.len()) }
                    else { format!("{} of {}", self.find.active.map_or(0, |i| i + 1), self.find.hits.len()) };
                let count_box = NodeSpec::row().min_width(if compact { 24.0 } else { 48.0 }).tooltip(&count);
                let count_box = if compact && !missing { count_box.max_width(58.0) } else { count_box };
                ui.text_in_keyed("find-count", count_box, &count,
                    TextStyle::new(Type::SMALL.size).color(if missing { t.danger } else { t.fg }).ellipsis());
                for (name, glyph, back) in [("Previous match", Glyph::ChevronUp, 1), ("Next match", Glyph::ChevronDown, 0)] {
                    if compact && self.find.hits.is_empty() { continue; }
                    ui.with_keyed(name, NodeSpec::row().size(if compact { 26.0 } else { 28.0 }, 30.0).radius(5.0)
                        .hover_bg(t.hover).center()
                        .on_click(Msg::FindStep { back }).disabled(self.find.hits.is_empty())
                        .opacity(if self.find.hits.is_empty() { t.disabled_opacity } else { 1.0 }).label(name)
                        .tooltip(if back == 1 { "Previous match · Shift+Enter" } else { "Next match · Enter" }),
                        |ui| icon(ui, glyph, t.fg, 14.0, false));
                }
                let done = NodeSpec::row().height(30.0).center().radius(5.0).hover_bg(t.hover)
                    .on_click(Msg::CloseFind).label("Close find").tooltip("Close find · Esc");
                if compact {
                    ui.with_keyed("Close find", done.width(26.0), |ui| icon(ui, Glyph::Cross, t.fg, 14.0, false));
                } else {
                    ui.text_in_keyed("Close find", done.pad_xy(8.0, 0.0), "Done", TextStyle::new(Type::SMALL.size).color(t.fg).nowrap());
                }
            });
        });
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_offsets_stay_in_the_source() {
        assert_eq!(ranges("Été ПрИвЕт İ after", "привет"), vec![(6, 18)]);
        assert_eq!(ranges("İ after", "after"), vec![(3, 8)]);
        assert!(ranges("İ", "i").is_empty(), "never match half a lowercase expansion");
        assert_eq!(ranges("İ", "i\u{307}"), vec![(0, 2)]);
        assert!(ranges("anything", "").is_empty());
    }

    #[test]
    fn previews_keep_the_matching_text_visible_at_narrow_widths() {
        let text = format!("{} Привет {}", "long context ".repeat(30), "following words ".repeat(20));
        let preview = excerpt(&text, "привет", 60.0, |s| s.chars().count() as f32);
        let hit = ranges(&preview, "привет")[0];
        assert!(preview[..hit.0].chars().count() <= 10);
        assert!(preview.chars().count() <= 60);
        assert!(preview.starts_with('…') && preview.ends_with('…'));
        // A query larger than the card is kept whole so wrapping can
        // reveal it instead of silently omitting the reason for the match.
        assert!(excerpt("before extraordinary after", "extraordinary", 8.0, |s| s.len() as f32).contains("extraordinary"));
    }

    #[test]
    fn matches_visible_words_across_styles_in_cells_and_code() {
        let doc = carnet_core::doc::from_markdown("Some **bold** words\n\n| Name |\n| --- |\n| CaT |\n\n```rs\ncat\n```\n\n[Label](https://hidden.example)").1;
        assert_eq!(body_hits(&doc, "bold words").count(), 1);
        let Hit::Body(c) = body_hits(&doc, "bold words").next().unwrap() else { panic!() };
        assert_eq!(&page::text(&doc, c.head.slot)[c.anchor.byte..c.head.byte], "bold** words");
        assert_eq!(body_hits(&doc, "cat").count(), 2);
        assert!(body_hits(&doc, "cat").next().is_some_and(|h| matches!(h, Hit::Body(c) if c.head.slot.cell.is_some())));
        assert!(!matches("Title", &doc, "hidden.example"));
        assert!(!matches("Title", &doc, "```rs"));
        assert!(matches("Title", &doc, "TITLE"));
    }
}
