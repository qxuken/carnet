//! Carnet's own tab in kui's devtools (F12 in a debug build): the app's
//! state at a glance — the library, the open note, the caret, the
//! pictures, the layout and the settings — next to kui's tree, events and
//! frame stats. Built only while the tab is on show.

use super::*;

impl Carnet {
    pub(super) fn devtools_tab(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let rows = self.devtools_rows(ui);
        let t = l.t;
        ui.devtools_tab_with("carnet", "Carnet", |ui| {
            ui.with(
                NodeSpec::column()
                    .fill()
                    .scroll_y()
                    .pad_xy(12.0, 10.0)
                    .gap(2.0)
                    .selectable(),
                |ui| {
                    for (i, (name, value)) in rows.iter().enumerate() {
                        if value.is_empty() {
                            // A section's heading.
                            ui.text_in_indexed(
                                i as u64,
                                NodeSpec::row().padding(Edges { l: 0.0, r: 0.0, t: if i > 0 { 10.0 } else { 0.0 }, b: 2.0 }),
                                name,
                                TextStyle::new(Type::SMALL.size).color(t.accent).nowrap(),
                            );
                            continue;
                        }
                        ui.with_indexed(i as u64, NodeSpec::row().grow_width().gap(8.0), |ui| {
                            ui.text_in(
                                NodeSpec::row().width(120.0),
                                name,
                                TextStyle::new(Type::SMALL.size).color(t.muted).nowrap(),
                            );
                            ui.text_in(
                                NodeSpec::row().grow_width().min_width(0.0),
                                value,
                                TextStyle::new(Type::SMALL.size).mono().color(t.fg),
                            );
                        });
                    }
                },
            );
        });
    }

    /// What the tab shows, as (name, value) rows; a row with no value is
    /// a section's heading.
    fn devtools_rows(&mut self, ui: &mut Ui<'_>) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = Vec::new();
        let head = |rows: &mut Vec<(String, String)>, s: &str| rows.push((s.to_string(), String::new()));
        let row = |rows: &mut Vec<(String, String)>, k: &str, v: String| rows.push((k.to_string(), v));

        head(&mut rows, "Library");
        let notes = &self.store.notes;
        row(&mut rows, "folder", self.store.dir.display().to_string());
        row(&mut rows, "notes", format!("{} ({} shown, {} pinned, {} unsaved)", notes.len(), self.order.len(),
            notes.iter().filter(|n| n.pinned).count(), notes.iter().filter(|n| n.dirty).count()));
        row(&mut rows, "folders", format!("{}", self.store.folders.len()));
        row(&mut rows, "showing", self.folder.clone().unwrap_or_else(|| "All notes".into()));
        row(&mut rows, "search", format!("{:?}", self.query));

        head(&mut rows, "Note");
        let note = &self.store.notes[self.current];
        let id = note.id;
        row(&mut rows, "title", note.display_title().to_string());
        row(&mut rows, "file", note.path.as_deref().map_or("(not saved yet)".into(), |p| p.display().to_string()));
        row(&mut rows, "id · rev", format!("{} · {}", note.id, note.rev));
        row(&mut rows, "blocks · slots", format!("{} · {}", note.doc.len(), page::slots(&note.doc).len()));
        row(&mut rows, "words", format!("{}", note.doc.word_count()));
        row(&mut rows, "dirty", format!("{}", note.dirty));
        let (undo, redo) = self.histories.get(&id).map_or((0, 0), History::depth);
        row(&mut rows, "history", format!("{undo} to undo, {redo} to redo"));

        head(&mut rows, "Caret");
        let c = self.caret;
        let slot = |s: Slot| match s.cell {
            Some((r, col)) => format!("block {} cell {r}:{col}", s.block),
            None => format!("block {}", s.block),
        };
        row(&mut rows, "head", format!("{} @ {}", slot(c.head.slot), c.head.byte));
        row(&mut rows, "anchor", format!("{} @ {}", slot(c.anchor.slot), c.anchor.byte));
        let cov = page::coverage(self.doc(), c);
        row(&mut rows, "selection", format!("{} whole, {} in part", cov.whole.len(), cov.parts.len()));
        row(&mut rows, "editing", format!("{}", self.editing));
        row(&mut rows, "focus", format!("{:?}", ui.focused()));

        head(&mut rows, "Pictures");
        let (loading, ready, failed) = self.images.counts();
        row(&mut rows, "cache", format!("{ready} shown, {loading} reading, {failed} failed"));
        row(&mut rows, "drop", format!("{:?}", self.drop_index));
        row(&mut rows, "dragging", self.note_drag.as_ref().map_or("no".into(), |d| format!("note {} → {:?}", d.id, d.target)));

        head(&mut rows, "Layout");
        let v = ui.viewport();
        row(&mut rows, "viewport", format!("{:.0} × {:.0} at {:.2}x", v.w, v.h, ui.core().scale()));
        row(&mut rows, "rows drawn", format!("{} blocks, {} cells", self.row_rects.len(), self.cell_rects.len()));
        row(&mut rows, "text width", format!("{:.0}", self.text_w));
        row(&mut rows, "strip", format!("{} tools in More, lights inset {:.0}", self.overflow.len(), self.lights));

        head(&mut rows, "Settings");
        let s = &self.settings;
        row(&mut rows, "palette", format!("{} · {:?}", s.palette.name(), s.theme));
        row(&mut rows, "type", format!("{}px · {} / {}", s.size,
            if s.font.is_empty() { crate::look::BODY_FACE } else { &s.font },
            if s.heading_font.is_empty() { crate::look::HEADING_FACE } else { &s.heading_font }));
        row(&mut rows, "page", format!("{:?} · {:?} · {:?}", s.width, s.density, s.sort));
        row(&mut rows, "file", s.file.as_deref().map_or("(none)".into(), |p| p.display().to_string()));
        rows
    }
}
