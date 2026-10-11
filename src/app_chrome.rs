//! The window's chrome: the library on the left (its search, its cards,
//! each card's menu) and the strip over the page — the toolbar and the
//! window's own buttons. A child of `app`, so it reads the app's state.

use super::*;
use crate::icons::icon;
use crate::keys;

impl Carnet {
    /// The library beside the page, in a pane that slides shut when it is
    /// hidden and open again when it is shown: its column keeps its width
    /// inside, so nothing in it squeezes as the pane goes.
    pub(super) fn library_pane(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let shown = self.side_shown();
        let key = ui.child_key("library-pane");
        let open = ui.layout_of(key).is_some_and(|r| r.w > 0.5);
        let dragging = self.library_from.is_some();
        ui.with_keyed(
            "library-pane",
            NodeSpec::column()
                .width(if shown { self.library_w() } else { 0.0 })
                .grow_height()
                .clip()
                .on_layout(Value::Null)
                // Following the pointer at once while its edge is dragged.
                .transition(if dragging { 0.0 } else { 220.0 }),
            |ui| {
                if shown || open {
                    self.sidebar(ui, l, false);
                }
                // Its edge: dragged, the library wider or narrower.
                if shown {
                    let t = l.t;
                    let h = ui.viewport().h;
                    ui.leaf_keyed(
                        "library-edge",
                        NodeSpec::row()
                            .size(6.0, h)
                            .float(FloatConfig::parent().inside(Align::End, Align::Start))
                            .bg(if dragging { t.border_strong } else { Color::TRANSPARENT })
                            .hover_bg(t.border)
                            .cursor(kui_native::CursorShape::EwResize)
                            .on_drag(Msg::LibraryDrag)
                            .label("Library width")
                            .tooltip("Drag to make the notes wider or narrower  \u{b7}  double-click for the usual width"),
                    );
                }
            },
        );
        if !shown {
            if open {
                // Its last frames: drawn until it has gone.
                ui.request_frame();
            } else {
                // Nothing of it is there to take a dragged file.
                self.card_rects.clear();
                self.folder_rects.clear();
            }
        }
    }

    /// The library. Beside the page on a wide window; on a narrow one it
    /// is the whole window (`full`), the window's own buttons in its top
    /// row, until a note is chosen.
    pub(super) fn sidebar(&mut self, ui: &mut Ui<'_>, l: &Look, full: bool) {
        let t = l.t;
        let column = NodeSpec::column().grow_height().bg(l.side);
        let column = if full { column.grow_width() } else { column.width(self.library_w()) };
        ui.with(column, |ui| {
            // The top of the column is window chrome: drag it to move. Its
            // left edge, the search's and the cards' text all fall at 26.
            ui.with(
                NodeSpec::row()
                    .grow_width()
                    .height(STRIP_H)
                    // Past the traffic lights on a Mac: they sit over this
                    // corner of the window.
                    .padding(Edges { l: size::GUTTER.max(self.lights), r: if full { 0.0 } else { 14.0 }, t: 0.0, b: 0.0 })
                    .cross_align(Align::Center)
                    // Its buttons have their shortcuts: out of the Tab ring.
                    .focus_region()
                    .window_drag_unless(self.menu_open),
                |ui| {
                    let heading = if self.versions.is_some() {
                        "History"
                    } else if self.deleted {
                        "Deleted"
                    } else {
                        self.folder.as_deref().map_or("Notes", folder_name)
                    };
                    ui.text_in(
                        NodeSpec::row().grow_width().min_width(0.0),
                        heading,
                        TextStyle::new(Type::HEADING.size).family(l.serif).color(t.fg).ellipsis(),
                    );
                    ui.with_keyed(
                        "new-folder",
                        NodeSpec::row()
                            .size(30.0, 30.0)
                            .radius(radius::ROUND)
                            .hover_bg(t.hover)
                            .pressed_bg(t.pressed)
                            .on_click(Msg::NewFolder)
                            .label("New folder")
                            .tooltip(&keys::hint("New folder", keys::NEW_FOLDER))
                            .center(),
                        |ui| icon(ui, Icon::FolderPlus, t.muted, 18.0, false),
                    );
                    ui.with_keyed(
                        "new-note",
                        NodeSpec::row()
                            .size(30.0, 30.0)
                            .radius(radius::ROUND)
                            .hover_bg(t.accent_soft)
                            .pressed_bg(t.accent_soft)
                            .on_click(Msg::NewNote)
                            .label("New note")
                            .tooltip(&keys::hint("New note", keys::NEW_NOTE))
                            .center(),
                        |ui| icon(ui, Icon::Plus, t.accent, 18.0, false),
                    );
                    if full {
                        gap(ui, 8.0);
                        widgets::window_buttons(ui);
                    }
                },
            );
            self.search(ui, l);
            self.folders(ui, l);
            if self.versions.is_some() {
                self.history_list(ui, l);
                self.sidebar_foot(ui, l);
                return;
            }
            if self.deleted {
                self.deleted_list(ui, l);
                self.sidebar_foot(ui, l);
                return;
            }
            if self.order.is_empty() {
                ui.with(
                    NodeSpec::column()
                        .grow_width()
                        .padding(Edges { l: 26.0, r: 20.0, t: 18.0, b: 0.0 })
                        .gap(4.0),
                    |ui| {
                        if self.query.trim().is_empty() {
                            ui.text("Nothing here yet.", TextStyle::new(Type::UI.size).color(t.muted));
                        } else {
                            ui.text(
                                &format!("No notes match \u{201c}{}\u{201d}", self.query.trim()),
                                TextStyle::new(Type::UI.size).color(t.fg),
                            );
                            ui.text(
                                "Press Enter to make it a note.",
                                TextStyle::new(Type::SMALL.size).color(t.muted),
                            );
                        }
                    },
                );
                ui.leaf(NodeSpec::row().grow_height());
                self.sidebar_foot(ui, l);
                return;
            }
            let now = SystemTime::now();
            let away = !ui.env().focused;
            let (order, notes, current) = (&self.order, &self.store.notes, self.current);
            // Every note at once (all of them, or a search): each card says
            // which folder it is in.
            let all = self.folder.is_none() || !self.query.trim().is_empty();
            // A file dragged onto a card: ringed while it rests there.
            let drag_card = self.file_drag.as_ref().and_then(|d| d.card).map(|(n, _)| n);
            let picked: Vec<u64> = self
                .picked
                .iter()
                .filter_map(|p| match p {
                    Pick::Note(id) if self.picked.len() > 1 => Some(*id),
                    _ => None,
                })
                .collect();
            let mut rects: Vec<(usize, Rect)> = Vec::new();
            widgets::uniform_list_with(
                ui,
                "library",
                NodeSpec::column()
                    .grow_width()
                    .grow_height()
                    .pad_xy(12.0, 6.0)
                    .scrollbar_width(4.0)
                    .role(Role::List)
                    .label("Notes"),
                order.len(),
                // Taller or shorter with the density: the air around a
                // card's text, never the text.
                crate::design::Density::library_row(l.d),
                |_| NodeSpec::column().pad_xy(0.0, 3.0),
                |ui, i| {
                    let n = order[i];
                    let note = &notes[n];
                    let selected = n == current;
                    let ago = short_ago(now, note.modified);
                    let snippet = note.doc.snippet();
                    let place = if all && !note.folder.is_empty() {
                        Some(folder_name(&note.folder))
                    } else {
                        None
                    };
                    let mut card = NodeSpec::column()
                        .grow_width()
                        .grow_height()
                        .pad_xy(14.0, 4.0)
                        .gap(3.0)
                        .radius(radius::CARD)
                        .main_align(Align::Center)
                        .on_layout(Value::Null)
                        .on_click(Msg::OpenNote { note: n as u64 })
                        // Dragged to a folder row: moved there.
                        .on_drag(Msg::CardDrag { note: note.id })
                        // Right-click: the row's own menu, opened by the
                        // app on the next frame (`pending_menu`).
                        .on_context_menu(Value::Int(note.id as i64))
                        .label(note.display_title())
                        .selected(selected)
                        .description(format!(
                            "{}{} · {}",
                            if note.pinned { "Pinned · " } else { "" },
                            ago,
                            snippet
                        ));
                    card = if drag_card == Some(n) {
                        // A file resting on it: about to open.
                        card.bg(t.accent_soft)
                    } else if picked.contains(&note.id) {
                        // Chosen with others: the selection's wash.
                        card.bg(t.selection)
                    } else if selected && away {
                        // The window in the background: the chosen note
                        // stays marked, quietly, as the search does.
                        card.bg(l.card.with_alpha(l.card.a * 0.35))
                    } else if selected {
                        card.bg(l.card)
                            .shadow_color(l.shadow)
                            .shadow_blur(16.0)
                            .shadow_y(3.0)
                    } else {
                        card.hover_bg(t.hover)
                    };
                    let ck = ui.child_key_indexed(0);
                    if let Some(r) = ui.layout_of(ck) {
                        rects.push((n, r));
                    }
                    ui.with_indexed(0, card, |ui| {
                        ui.with(
                            NodeSpec::row().grow_width().gap(6.0).cross_align(Align::Center),
                            |ui| {
                                if note.pinned {
                                    ui.leaf(NodeSpec::row().size(6.0, 6.0).radius(3.0).bg(t.accent));
                                }
                                ui.text_in(
                                    NodeSpec::row().grow_width(),
                                    note.display_title(),
                                    TextStyle::new(Type::STRONG.size).color(t.fg).ellipsis(),
                                );
                                ui.text(&ago, TextStyle::new(Type::CAPTION.size).color(t.muted).nowrap());
                            },
                        );
                        // The folder, then the preview's pieces, drawn dots
                        // between them: a typed "·" sits wherever the
                        // font's metrics put it.
                        let parts = note.doc.snippet_parts();
                        let style = TextStyle::new(Type::SMALL.size).color(t.muted);
                        let mut pieces: Vec<(&str, Color)> = Vec::new();
                        if let Some(place) = place {
                            // The accent deepened for text: 5:1, where the accent is 2.9.
                            pieces.push((place, t.focus_ring));
                        }
                        if parts.is_empty() {
                            pieces.push(("No text", t.faint));
                        }
                        pieces.extend(parts.iter().map(|p| (p.as_str(), t.muted)));
                        ui.with(
                            NodeSpec::row().grow_width().gap(7.0).cross_align(Align::Center).clip(),
                            |ui| {
                                let last = pieces.len().saturating_sub(1);
                                for (k, (text, ink)) in pieces.iter().enumerate() {
                                    if k > 0 {
                                        // Centred on the lowercase letters,
                                        // a pixel under the line's middle.
                                        ui.with(
                                            NodeSpec::row().size(3.0, 5.0).padding(Edges { l: 0.0, r: 0.0, t: 2.0, b: 0.0 }).role(Role::None),
                                            |ui| {
                                                ui.leaf(NodeSpec::row().size(3.0, 3.0).radius(1.5).bg(t.faint));
                                            },
                                        );
                                    }
                                    if k == last {
                                        ui.text_in(
                                            NodeSpec::row().grow_width().min_width(0.0),
                                            text,
                                            style.color(*ink).ellipsis(),
                                        );
                                    } else {
                                        ui.text(text, style.color(*ink).nowrap());
                                    }
                                }
                            },
                        );
                    });
                },
            );
            self.card_rects = rects;
            self.sidebar_foot(ui, l);
        });
    }

    /// The Deleted folder: every note and folder in the trash, the latest
    /// first, each opened by a click to read, put back or delete forever.
    fn deleted_list(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let t = l.t;
        let now = SystemTime::now();
        ui.with_keyed(
            "deleted",
            NodeSpec::column()
                .grow_width()
                .grow_height()
                .pad_xy(12.0, 6.0)
                .gap(2.0)
                .scroll_y()
                .scrollbar_width(4.0)
                .role(Role::List)
                .label("Deleted"),
            |ui| {
                ui.text_in(
                    NodeSpec::row().pad_xy(14.0, 4.0),
                    "Click one to read it, then restore it or delete it forever.",
                    TextStyle::new(Type::SMALL.size).color(t.muted),
                );
                let open = self.peek.as_ref().map(|p| p.path.clone());
                for (i, d) in self.trash.iter().enumerate() {
                    let from = if d.folder.is_empty() { "All notes".to_string() } else { folder_name(&d.folder).to_string() };
                    let on = open.as_ref() == Some(&d.path);
                    ui.with_indexed(
                        i as u64,
                        NodeSpec::column()
                            .grow_width()
                            .pad_xy(14.0, 9.0)
                            .gap(3.0)
                            .radius(radius::CARD)
                            .bg(if on { t.accent_soft } else { Color::TRANSPARENT })
                            .hover_bg(if on { t.accent_soft } else { t.hover })
                            .on_click(Msg::OpenDeleted { index: i as u64 })
                            .on_context_menu(Value::str(format!("deleted:{i}")))
                            .label(d.title.clone())
                            .description(format!("Deleted {}, from {from} · {}", ago(now, d.when), d.snippet))
                            .selected(on),
                        |ui| {
                            ui.with(NodeSpec::row().grow_width().gap(6.0).cross_align(Align::Center), |ui| {
                                if d.is_folder {
                                    icon(ui, Icon::Folder, t.muted, 14.0, false);
                                }
                                ui.text_in(
                                    NodeSpec::row().grow_width().min_width(0.0),
                                    &d.title,
                                    TextStyle::new(Type::STRONG.size).color(t.fg).ellipsis(),
                                );
                                ui.text(&short_ago(now, d.when), TextStyle::new(Type::CAPTION.size).color(t.muted).nowrap());
                            });
                            let snippet = if d.snippet.is_empty() { "No text" } else { d.snippet.as_str() };
                            ui.text_in(
                                NodeSpec::row().grow_width().min_width(0.0),
                                snippet,
                                TextStyle::new(Type::SMALL.size).color(t.muted).ellipsis(),
                            );
                        },
                    );
                }
            },
        );
    }

    /// The open note's versions, newest first, as the Deleted folder lists
    /// what is in the trash: what made each, when, and how many words it
    /// added and took away. A click shows one on the page.
    fn history_list(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let t = l.t;
        let Some(v) = &self.versions else { return };
        let (rows, shown) = (v.list.clone(), v.shown);
        // Words in and out of each version, against the one before it.
        let deltas: Vec<Option<(usize, usize)>> = (0..rows.len())
            .map(|i| {
                let new = self.version_text(i)?;
                let old = if i + 1 < rows.len() { self.version_text(i + 1)? } else { String::new() };
                Some(carnet_core::compare::word_delta(&old, &new))
            })
            .collect();
        ui.with_keyed(
            "history",
            NodeSpec::column()
                .grow_width()
                .grow_height()
                .pad_xy(12.0, 6.0)
                .gap(2.0)
                .scroll_y()
                .scrollbar_width(4.0)
                .role(Role::List)
                .label("History"),
            |ui| {
                ui.text_in(
                    NodeSpec::row().pad_xy(14.0, 4.0),
                    "Click one to read it. Esc goes back to the note.",
                    TextStyle::new(Type::SMALL.size).color(t.muted),
                );
                for (i, r) in rows.iter().enumerate() {
                    let what = self.version_label(r);
                    let when = super::revision_ago(r);
                    let words = match deltas[i] {
                        Some((0, 0)) | None => "no words changed".to_string(),
                        Some((added, 0)) => format!("+{added} words"),
                        Some((0, gone)) => format!("−{gone} words"),
                        Some((added, gone)) => format!("+{added} −{gone} words"),
                    };
                    let detail = if i == 0 { format!("The note as it is · {words}") } else { format!("{when} · {words}") };
                    let mut row = NodeSpec::column()
                        .grow_width()
                        .pad_xy(14.0, 9.0)
                        .gap(3.0)
                        .radius(radius::CARD)
                        .on_click(Msg::ShowVersion { index: i as u64 })
                        .label(what.as_str())
                        .description(detail.clone())
                        .selected(i == shown);
                    row = if i == shown {
                        row.bg(l.card).shadow_color(l.shadow).shadow_blur(16.0).shadow_y(3.0)
                    } else {
                        row.hover_bg(t.hover)
                    };
                    ui.with_indexed(i as u64, row, |ui| {
                        ui.with(NodeSpec::row().grow_width().gap(6.0).cross_align(Align::Center), |ui| {
                            ui.text_in(
                                NodeSpec::row().grow_width().min_width(0.0),
                                &what,
                                TextStyle::new(Type::STRONG.size).color(t.fg).ellipsis(),
                            );
                            let age = if i == 0 { "now".to_string() } else { short_ago(SystemTime::now(), super::revision_time(r)) };
                            ui.text(&age, TextStyle::new(Type::CAPTION.size).color(t.muted).nowrap());
                        });
                        ui.text_in(
                            NodeSpec::row().grow_width().min_width(0.0),
                            &detail,
                            TextStyle::new(Type::SMALL.size).color(t.muted).ellipsis(),
                        );
                    });
                }
            },
        );
    }

    /// The column's last row: the way to the settings.
    fn sidebar_foot(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let t = l.t;
        // Out of the Tab ring (it has its shortcut): Tab goes from the
        // search through the notes to the note.
        ui.with(NodeSpec::row().grow_width().pad_xy(12.0, 8.0).focus_region(), |ui| {
            ui.with_keyed(
                "open-settings",
                NodeSpec::row()
                    .height(34.0)
                    .padding(Edges { l: 12.0, r: 14.0, t: 0.0, b: 0.0 })
                    .gap(9.0)
                    .radius(radius::CONTROL)
                    .cross_align(Align::Center)
                    .bg(if self.settings_open { t.accent_soft } else { Color::TRANSPARENT })
                    .hover_bg(if self.settings_open { t.accent_soft } else { t.hover })
                    .on_click(Msg::ShowSettings)
                    .label("Settings")
                    .tooltip(&keys::hint("Settings", keys::SETTINGS)),
                |ui| {
                    icon(ui, Icon::Settings, t.muted, 16.0, false);
                    ui.text("Settings", TextStyle::new(Type::UI.size).color(t.muted).nowrap());
                },
            );
        });
    }

    /// The folders, under the search: "All notes", then each folder at its
    /// depth with how many notes it holds. Shown once there is a folder (or
    /// one is being named); a click shows its notes, a right-click its menu.
    fn folders(&mut self, ui: &mut Ui<'_>, l: &Look) {
        if self.store.folders.is_empty() && self.naming.is_none() && self.trash.is_empty() {
            self.folder_rects.clear();
            return;
        }
        let t = l.t;
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for n in &self.store.notes {
            *counts.entry(n.folder.as_str()).or_default() += 1;
        }
        // A note carried over a row: it lights up to take it.
        let hover = self.note_drag.as_ref().and_then(|d| d.target.clone());
        let mut rects: Vec<(String, Rect)> = Vec::new();
        let row = |depth: usize, selected: bool| {
            let spec = NodeSpec::row()
                .grow_width()
                .height(30.0)
                .padding(Edges { l: 14.0 + depth as f32 * 14.0, r: 12.0, t: 0.0, b: 0.0 })
                .gap(9.0)
                .radius(radius::CONTROL)
                .cross_align(Align::Center)
                .selected(selected);
            if selected { spec.bg(t.accent_soft) } else { spec.hover_bg(t.hover) }
        };
        let label = |ui: &mut Ui<'_>, name: &str, count: usize, selected: bool| {
            ui.text_in(
                NodeSpec::row().grow_width().min_width(0.0),
                name,
                TextStyle::new(Type::UI.size).color(if selected { t.fg } else { t.muted }).ellipsis(),
            );
            ui.text(&count.to_string(), TextStyle::new(Type::CAPTION.size).color(t.muted).nowrap());
        };
        let naming = self.naming.clone();
        let mut name_key = None;
        // The rows in order, for the keys pressed on them.
        let mut rows: Vec<(Key, FolderRow)> = Vec::new();
        ui.with_keyed(
            "folders",
            NodeSpec::column()
                .grow_width()
                .max_height(212.0)
                .scroll_y()
                .scrollbar_width(4.0)
                .pad_xy(12.0, 2.0)
                .role(Role::List)
                .label("Folders"),
            |ui| {
                let all = self.folder.is_none() && !self.deleted;
                if let Some(r) = ui.layout_of(ui.child_key("f-all")) {
                    rects.push((String::new(), r));
                }
                let lit = |spec: NodeSpec, f: &str| {
                    let chosen = !f.is_empty() && self.is_picked(&Pick::Folder(f.to_string()));
                    if hover.as_deref() == Some(f) || chosen { spec.bg(t.selection) } else { spec }
                };
                let k = ui.with_keyed(
                    "f-all",
                    lit(row(0, all), "")
                        .on_click(Msg::SelectFolder { index: ALL })
                        .on_context_menu(Value::str("folder:all"))
                        .on_layout(Value::Null)
                        .label("All notes"),
                    |ui| {
                        icon(ui, Icon::Folder, if all { t.accent } else { t.muted }, 16.0, false);
                        label(ui, "All notes", self.store.notes.len(), all);
                    },
                );
                rows.push((k, FolderRow::All));
                // Each naming its own field, so it opens empty; kui keeps a
                // field's text while it is away, and a name dropped with
                // Escape would come back in the next one.
                // A rename's field opens on the folder's name.
                let field = |ui: &mut Ui<'_>, depth: usize, n: &Naming| -> Key {
                    let style = TextStyle::new(Type::UI.size).color(t.fg);
                    let mut k = Key::ROOT;
                    let (initial, said) = match &n.rename {
                        Some(f) => (folder_name(f), "Folder's new name"),
                        None => ("", "New folder's name"),
                    };
                    ui.with_keyed("f-name", row(depth, false).bg(l.card), |ui| {
                        icon(ui, Icon::Folder, t.accent, 16.0, false);
                        k = ui.text_edit(
                            &format!("folder-name-{}", n.nth),
                            initial,
                            &EditOptions { style, placeholder: Some("Folder name".into()), ..Default::default() },
                            NodeSpec::row().grow_width().label(said),
                        );
                    });
                    k
                };
                for (i, f) in self.store.folders.iter().enumerate() {
                    let depth = f.matches('/').count() + 1;
                    let selected = !self.deleted && self.folder.as_deref() == Some(f.as_str());
                    if let Some(r) = ui.layout_of(ui.child_key_indexed(i as u64)) {
                        rects.push((f.clone(), r));
                    }
                    // Being renamed: its field in its place.
                    if let Some(n) = naming.as_ref().filter(|n| n.rename.as_deref() == Some(f.as_str())) {
                        name_key = Some(field(ui, depth.min(4), n));
                        continue;
                    }
                    let k = ui.with_indexed(
                        i as u64,
                        lit(row(depth.min(4), selected), f)
                            .on_layout(Value::Null)
                            .on_click(Msg::SelectFolder { index: i as u64 })
                            .on_context_menu(Value::str(format!("folder:{i}")))
                            .label(folder_name(f)),
                        |ui| {
                            icon(ui, Icon::Folder, if selected { t.accent } else { t.muted }, 16.0, false);
                            label(ui, folder_name(f), counts.get(f.as_str()).copied().unwrap_or(0), selected);
                        },
                    );
                    rows.push((k, FolderRow::Folder(f.clone())));
                    // A subfolder being named goes under its parent.
                    if let Some(n) = &naming
                        && n.rename.is_none()
                        && n.parent == *f
                    {
                        name_key = Some(field(ui, (depth + 1).min(4), n));
                    }
                }
                if let Some(n) = &naming
                    && n.rename.is_none()
                    && n.parent.is_empty()
                {
                    name_key = Some(field(ui, 1, n));
                }
                // Last, what was deleted: all of it, for as long as it is
                // in the trash.
                if !self.trash.is_empty() {
                    let on = self.deleted;
                    let k = ui.with_keyed(
                        "f-deleted",
                        row(0, on).on_click(Msg::ShowDeleted).label("Deleted"),
                        |ui| {
                            icon(ui, Icon::Trash, if on { t.accent } else { t.muted }, 16.0, false);
                            label(ui, "Deleted", self.trash.len(), on);
                        },
                    );
                    rows.push((k, FolderRow::Deleted));
                }
            },
        );
        self.folder_rects = rects;
        // The row the keyboard was sent to, now that it is drawn.
        if let Some(want) = self.focus_row.take()
            && let Some((k, _)) = rows.iter().find(|(_, r)| *r == want)
        {
            ui.focus(*k);
            ui.reveal(*k);
        }
        self.folder_rows = rows;
        if let Some(k) = name_key {
            self.fields.insert(k, Field::Folder);
            if let Some(n) = &mut self.naming {
                n.key = Some(k);
            }
            self.apply_focus(ui, k, Field::Folder);
        }
    }

    /// A pill with no chrome: a drawn lens, the field ("Search" while it is
    /// empty), and a clear button once there is something to clear.
    pub(super) fn search(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let t = l.t;
        let empty = self.query.is_empty();
        let clear = std::mem::take(&mut self.clear_search);
        // Quiet until it is used: in a window in the background, or with
        // nothing searched and the keyboard elsewhere.
        let quiet = !ui.env().focused || (self.query.is_empty() && !self.search_active);
        ui.with(NodeSpec::row().grow_width().pad_xy(12.0, 8.0), |ui| {
            ui.with_keyed(
                "search-pill",
                NodeSpec::row()
                    .grow_width()
                    .height(34.0)
                    .padding(Edges { l: 14.0, r: 6.0, t: 0.0, b: 0.0 })
                    .gap(8.0)
                    .radius(radius::ROUND)
                    // Not in use, the field steps back into the library, as
                    // a Mac's controls do in a window in the background: a
                    // solid pill there is the loudest thing on the screen.
                    // A click, ⌘F or a search brings it whole.
                    .bg(if quiet { l.pill.with_alpha(l.pill.a * 0.35) } else { l.pill })
                    .transition(160.0)
                    .cross_align(Align::Center)
                    .on_click(Msg::FocusSearch)
                    // A click target for the mouse, not a second Tab stop
                    // in front of the field.
                    .role(Role::Group),
                |ui| {
                    icon(ui, Icon::Search, t.muted, 15.0, false);
                    let k = ui.text_edit(
                        "search",
                        "",
                        &EditOptions {
                            style: TextStyle::new(Type::UI.size).color(t.fg),
                            placeholder: Some("Search".into()),
                            ..Default::default()
                        },
                        NodeSpec::row().grow_width().label("Search notes"),
                    );
                    if clear {
                        ui.set_edit_text(k, "");
                    }
                    self.fields.insert(k, Field::Search);
                    self.apply_focus(ui, k, Field::Search);
                    if !empty {
                        ui.with_keyed(
                            "clear-search",
                            NodeSpec::row()
                                .size(22.0, 22.0)
                                .radius(radius::ROUND)
                                .hover_bg(t.hover)
                                .on_click(Msg::ClearSearch)
                                .label("Clear search")
                                .center(),
                            |ui| icon(ui, Icon::Cross, t.muted, 14.0, false),
                        );
                    }
                },
            );
        });
    }

    /// The strip over the page: the window's drag handle, the block tools,
    /// and the window's own buttons. Every tool has a fixed place; the
    /// slot in the middle swaps list tools for table tools at one width.
    /// On a wide window the tools start where the page's text does; on a
    /// narrow one they give way — clipped at their end, never the window's
    /// buttons — and a button brings back the library.
    pub(super) fn strip(&mut self, ui: &mut Ui<'_>, l: &Look, narrow: bool) {
        let t = l.t;
        let doc = self.doc();
        // A version in the history is read, not written: no tools for it.
        let editing = self.editing && !self.reading();
        let kinds: Vec<Kind> = if editing {
            page::touched(doc, self.caret)
                .map(|i| doc.blocks()[i].kind.clone())
                .collect()
        } else {
            Vec::new()
        };
        let single = kinds.len() == 1;
        let in_table = editing && self.caret.head.slot.cell.is_some();
        let convertible = !in_table && kinds.iter().any(|k| page::turned(k, TURN_TEXT).is_some());
        let is_item = kinds.iter().any(|k| matches!(k, Kind::Item { .. }));
        let pinned = self.store.notes[self.current].pinned;
        let on = |to: u64| -> bool {
            if !single {
                return false;
            }
            match (&kinds[0], to) {
                (Kind::Text(_), TURN_TEXT) => true,
                (Kind::Heading { level, .. }, l) => (*level as u64).min(3) == l,
                (Kind::Item { style, .. }, TURN_BULLET) => *style == ListStyle::Bullet,
                (Kind::Item { style, .. }, TURN_NUMBER) => *style == ListStyle::Number,
                (Kind::Item { style, .. }, TURN_TODO) => *style == ListStyle::Todo,
                (Kind::Quote(_), TURN_QUOTE) => true,
                _ => false,
            }
        };
        let sorted = match (in_table, self.caret.head.slot.cell, self.last_sort) {
            (true, Some((_, col)), Some((c, desc))) if c == col => Some(desc),
            _ => None,
        };
        let vw = ui.viewport().w;
        let main_w = vw - self.side_w(narrow);
        let lead = ((main_w - l.page_w).max(0.0) / 2.0 + 48.0).max(12.0);
        // The tools, in groups that show whole or go to the More menu
        // whole: what the block becomes, its styles, what to insert, and
        // the slot that changes with what the caret is in.
        let def = |key: &'static str, icon: Icon, name: &'static str, hint: String, msg: Msg| ToolDef {
            key,
            icon,
            name,
            hint,
            spec: None,
            msg,
            enabled: true,
            toggle: None,
            group: 0,
        };
        let mut groups: Vec<Vec<ToolDef>> = Vec::new();
        let mut turn = Vec::new();
        for (key, icon, name, typed, spec, to) in [
            ("t-text", Icon::Text, "Text", "", keys::TEXT, TURN_TEXT),
            ("t-h1", Icon::H1, "Heading 1", "# ", keys::H1, TURN_H1),
            ("t-h2", Icon::H2, "Heading 2", "## ", keys::H2, TURN_H2),
            ("t-h3", Icon::H3, "Heading 3", "### ", keys::H3, TURN_H3),
            ("t-bullet", Icon::Bullets, "Bulleted list", "- ", keys::BULLETS, TURN_BULLET),
            ("t-number", Icon::Numbers, "Numbered list", "1. ", keys::NUMBERS, TURN_NUMBER),
            ("t-todo", Icon::Todo, "Checklist", "[] ", keys::TODO, TURN_TODO),
            ("t-quote", Icon::Quote, "Quote", "> ", keys::QUOTE, TURN_QUOTE),
        ] {
            let mut hint = keys::hint(name, spec);
            if !typed.is_empty() {
                hint.push_str(&format!("  \u{b7}  {typed}"));
            }
            let mut d = def(key, icon, name, hint, Msg::Turn { to });
            d.spec = Some(spec);
            d.enabled = convertible;
            d.toggle = Some(on(to));
            turn.push(d);
        }
        // A paragraph's first letter dropped, a few lines tall.
        let initial = page::initial_at(self.doc(), self.caret);
        let mut d = def(
            "t-initial",
            Icon::Initial,
            "Drop the first letter",
            keys::hint("Drop the first letter", keys::INITIAL),
            Msg::ToggleInitial,
        );
        d.spec = Some(keys::INITIAL);
        d.enabled = editing && initial.is_some();
        d.toggle = Some(initial == Some(true));
        turn.push(d);
        groups.push(turn);
        let mut styles = Vec::new();
        let caret_style = self.writing.style_at(self.doc(), self.caret);
        for (key, icon, name, spec, which) in [
            ("t-bold", Icon::Bold, "Bold", keys::BOLD, 0),
            ("t-italic", Icon::Italic, "Italic", keys::ITALIC, 1),
            ("t-strike", Icon::Strike, "Strikethrough", keys::STRIKE, 2),
            ("t-code", Icon::Code, "Code", keys::CODE, 3),
        ] {
            let mut d = def(key, icon, name, keys::hint(name, spec), Msg::Style { which });
            d.spec = Some(spec);
            d.enabled = editing;
            // On where the caret's words have it, or where it is switched
            // on for what is typed next.
            let kind = crate::app::STYLES[which as usize];
            let pending = self
                .writing
                .pending_at(self.caret)
                .and_then(|p| p.layers.iter().find(|l| l.0 == format!("style{which}")));
            d.toggle = Some(match pending {
                Some(l) => l.1 == kind.marker(),
                None => editing && kind.on(&caret_style),
            });
            styles.push(d);
        }
        let mut d = def("t-mark", Icon::Highlight, "Highlight", format!("Highlight  \u{b7}  {} for yellow", keys::label(keys::HIGHLIGHT)), Msg::TintMenu { ink: 0 });
        d.spec = Some(keys::HIGHLIGHT);
        d.enabled = editing;
        d.toggle = Some(self.tint_open.is_some_and(|(ink, _)| !ink));
        styles.push(d);
        let mut d = def("t-color", Icon::TextColor, "Text colour", "Text colour".into(), Msg::TintMenu { ink: 1 });
        d.enabled = editing;
        d.toggle = Some(self.tint_open.is_some_and(|(ink, _)| ink));
        styles.push(d);
        groups.push(styles);
        let mut insert = vec![
            def("t-table", Icon::Table, "Table", keys::hint("Table", keys::TABLE), Msg::InsertTable),
            def("t-rule", Icon::Divider, "Divider", keys::hint("Divider", keys::DIVIDER) + "  \u{b7}  ---", Msg::InsertDivider),
        ];
        insert[0].spec = Some(keys::TABLE);
        insert[1].spec = Some(keys::DIVIDER);
        groups.push(insert);
        let mut slot = Vec::new();
        if in_table {
            slot.push(def("t-addrow", Icon::AddRow, "Add row", "Add a row below  \u{b7}  Enter at the bottom".into(), Msg::AddRow));
            slot.push(def("t-addcol", Icon::AddCol, "Add column", "Add a column to the right".into(), Msg::AddCol));
            slot.push(def("t-delrow", Icon::DelRow, "Remove row", "Remove this row".into(), Msg::DelRow));
            slot.push(def("t-delcol", Icon::DelCol, "Remove column", "Remove this column".into(), Msg::DelCol));
            let (glyph, name) = match sorted {
                Some(false) => (Icon::SortUp, "Sort Z to A"),
                Some(true) => (Icon::SortDown, "Sort A to Z"),
                None => (Icon::Sort, "Sort by this column"),
            };
            slot.push(def("t-sort", glyph, name, "Sort by this column (again to reverse)".into(), Msg::Sort));
            let mut d = def("t-del", Icon::Cross, "Remove table", keys::hint("Remove the table", keys::REMOVE), Msg::DeleteBlock);
            d.spec = Some(keys::REMOVE);
            slot.push(d);
        } else if editing {
            // A picture: where it sits, beside the words or on its own line.
            if let Some((_, float, _)) = self.picture_here() {
                for (key, icon, name, spec, to) in [
                    ("t-inline", Icon::Inline, "On its own line", None, carnet_core::doc::Float::None),
                    ("t-left", Icon::FloatLeft, "Left of the text", Some(keys::FLOAT_LEFT), carnet_core::doc::Float::Left),
                    ("t-right", Icon::FloatRight, "Right of the text", Some(keys::FLOAT_RIGHT), carnet_core::doc::Float::Right),
                ] {
                    let hint = spec.map_or_else(|| name.to_string(), |s| keys::hint(name, s));
                    let mut d = def(key, icon, name, hint, Msg::Float { to: to as u64 });
                    d.spec = spec;
                    d.toggle = Some(float == to);
                    slot.push(d);
                }
            }
            if is_item {
                let mut d = def("t-out", Icon::Outdent, "Outdent", keys::hint("Outdent", keys::OUTDENT), Msg::Indent { by: -1 });
                d.spec = Some(keys::OUTDENT);
                slot.push(d);
                let mut d = def("t-in", Icon::Indent, "Indent", keys::hint("Indent", keys::INDENT), Msg::Indent { by: 1 });
                d.spec = Some(keys::INDENT);
                slot.push(d);
            }
            let mut d = def("t-up", Icon::Up, "Move up", keys::hint("Move up", keys::MOVE_UP), Msg::Move { by: -1 });
            d.spec = Some(keys::MOVE_UP);
            slot.push(d);
            let mut d = def("t-down", Icon::Down, "Move down", keys::hint("Move down", keys::MOVE_DOWN), Msg::Move { by: 1 });
            d.spec = Some(keys::MOVE_DOWN);
            slot.push(d);
            let mut d = def("t-del", Icon::Cross, "Remove block", keys::hint("Remove", keys::REMOVE), Msg::DeleteBlock);
            d.spec = Some(keys::REMOVE);
            slot.push(d);
        }
        if !slot.is_empty() {
            groups.push(slot);
        }
        // A picture selected: what a block becomes and the words' styles
        // do nothing to it, so its own tools come first in their place.
        if editing && self.picture_here().is_some() {
            groups.drain(..2);
            groups.rotate_right(1);
        }
        // The tools start where the text does, less what the strip's
        // start already holds.
        let side = self.side_shown();
        let held = if side { 8.0 } else { 8.0_f32.max(self.lights) } + 61.0;
        let pad_l = if narrow { 4.0 } else { (lead - held).max(8.0) };
        let lights = self.lights;
        let menu_open = self.menu_open;
        // While words are going down the tools step out of the way: slowly,
        // so the page is not seen to change, and back at once when wanted.
        // In zen they stay away until the pointer comes up to the strip.
        let near = ui.core().cursor().is_some_and(|p| p.y <= STRIP_H + 4.0);
        let busy = self.menu_open || self.tint_open.is_some() || self.link_edit.is_some();
        let calm = (self.settings.fade_tools && self.in_flow()) || (self.zen && !near && !busy);
        let mut hidden: Vec<ToolDef> = Vec::new();
        let mut more = None;
        let mut anchors = [None, None];
        // The tools are the pointer's (each has its shortcut, and a screen
        // reader reaches them): out of the Tab ring, which goes search,
        // notes, title, note. The narrow strip's way back stays in it.
        let mut strip = NodeSpec::row().grow_width().height(STRIP_H).cross_align(Align::Center);
        if !narrow {
            strip = strip.focus_region();
        }
        ui.with(
            strip.window_drag_unless(menu_open),
            |ui| {
                if narrow {
                    gap(ui, 8.0_f32.max(lights));
                    Tool::new("t-back", Icon::Back, "All notes", "Back to the list of notes".to_string(), Msg::ShowList)
                        .draw(ui, &t);
                } else {
                    // The library's own switch, at the strip's start: past
                    // the traffic lights when the library has gone.
                    gap(ui, if side { 8.0 } else { 8.0_f32.max(lights) });
                    Tool::new(
                        "t-sidebar",
                        Icon::Sidebar,
                        if side { "Hide the notes" } else { "Show the notes" },
                        keys::hint(if side { "Hide the notes" } else { "Show the notes" }, keys::SIDEBAR),
                        Msg::ToggleSidebar,
                    )
                    .calm(calm)
                    .draw(ui, &t);
                }
                Tool::new("t-zen", Icon::Zen, "Zen mode", keys::hint("Zen mode", keys::ZEN), Msg::ToggleZen)
                    .toggle(self.zen)
                    .calm(calm)
                    .draw(ui, &t);
                // How many tools fit, in order: the row's width as last
                // laid out. A tool is 30 with 1 between; groups 10 apart
                // (+2). When not all fit, room is kept for More.
                let tools_key = ui.child_key("tools");
                let avail = ui.layout_of(tools_key).map_or(f32::MAX, |r| r.w - pad_l - 8.0);
                const SEP: f32 = 12.0;
                let count: usize = groups.iter().map(Vec::len).sum();
                let width_of = |n: usize| -> f32 {
                    // The first `n` tools, with the group gaps among them.
                    let (mut w, mut left) = (0.0, n);
                    for (i, g) in groups.iter().enumerate() {
                        if left == 0 {
                            break;
                        }
                        let k = g.len().min(left);
                        w += if i > 0 { SEP } else { 0.0 } + k as f32 * 31.0 - 1.0;
                        left -= k;
                    }
                    w
                };
                let shown = if width_of(count) <= avail {
                    count
                } else {
                    (0..count).rev().find(|&n| width_of(n) + SEP + 30.0 <= avail).unwrap_or(0)
                };
                ui.with_keyed(
                    "tools",
                    NodeSpec::row()
                        .grow_width()
                        .min_width(0.0)
                        .height(STRIP_H)
                        .padding(Edges { l: pad_l, r: 8.0, t: 0.0, b: 0.0 })
                        .gap(1.0)
                        .cross_align(Align::Center)
                        .on_layout(Value::Null)
                        .clip(),
                    |ui| {
                        let mut n = 0;
                        for (i, g) in groups.drain(..).enumerate() {
                            for (j, mut d) in g.into_iter().enumerate() {
                                d.group = i;
                                if n >= shown {
                                    hidden.push(d);
                                    continue;
                                }
                                if i > 0 && j == 0 {
                                    gap(ui, 10.0);
                                }
                                n += 1;
                                // The colour picker hangs from its button.
                                if let Some(i) = ["t-mark", "t-color"].iter().position(|k| *k == d.key) {
                                    anchors[i] = ui.layout_of(ui.child_key(d.key)).map(|r| Vec2::new(r.x, r.y + r.h + 4.0));
                                }
                                let mut tool = Tool::new(d.key, d.icon, d.name, d.hint, d.msg).enabled(d.enabled).calm(calm);
                                if let Some(on) = d.toggle {
                                    tool = tool.toggle(on);
                                }
                                tool.draw(ui, &t);
                            }
                        }
                        if !hidden.is_empty() {
                            if shown > 0 {
                                gap(ui, 10.0);
                            }
                            let k = ui.child_key("t-more");
                            more = ui.layout_of(k).map(|r| (k, Vec2::new(r.x, r.y + r.h + 4.0)));
                            Tool::new("t-more", Icon::More, "More tools", "More tools".to_string(), Msg::ToolsMenu)
                                .calm(calm)
                                .draw(ui, &t);
                        }
                    },
                );
                // A deleted note on the page has its own ways out, in its
                // line: these are for a note in the list.
                if self.peek.is_none() {
                    Tool::new(
                        "t-pin",
                        Icon::Star,
                        "Pin note",
                        keys::hint(if pinned { "Unpin" } else { "Pin to the top" }, keys::PIN),
                        Msg::PinNote,
                    )
                    .toggle(pinned)
                    .filled(pinned)
                    .calm(calm)
                    .draw(ui, &t);
                    // Apart a little: one keeps a note, the other throws it away.
                    gap(ui, 6.0);
                    Tool::new("t-trash", Icon::Trash, "Delete note", keys::hint("Move this note to the trash", keys::TRASH), Msg::DeleteNote)
                        .calm(calm)
                        .draw(ui, &t);
                }
                gap(ui, 8.0);
                widgets::window_buttons(ui);
            },
        );
        self.overflow = hidden;
        self.more = more;
        self.tint_anchor = anchors;
        // Hanging from its button wherever the strip put it this time.
        if let (Some((_, at)), Some(Some(a))) = (self.tint_open.as_mut(), self.tint_from.map(|i| anchors[i])) {
            *at = a;
        }
    }

    /// The colour picker, hanging from its button: the grid Word and
    /// Obsidian's toolbar share — ten theme colours over five lighter and
    /// darker steps of each, ten standard ones, the reader's recent ones —
    /// and a way back to none. One picker, two tabs: the words' colour and
    /// the highlighter's.
    pub(super) fn tint_picker(&self, ui: &mut Ui<'_>, l: &Look) {
        let Some((ink, at)) = self.tint_open else { return };
        let t = l.t;
        // The colour the caret's words have now: ringed in the grid.
        let now = {
            let p = self.caret.head;
            let text = page::text(self.doc(), p.slot);
            let st = carnet_core::inline::styles(text);
            let i = p.byte.min(st.len()).saturating_sub(if p.byte > 0 && p.byte == st.len() { 1 } else { 0 });
            st.get(i).map_or(0, |s| if ink { s.ink } else { s.mark })
        };
        let k = ink as u64;
        let label = |ui: &mut Ui<'_>, s: &str| {
            ui.text(s, TextStyle::new(Type::SMALL.size).color(t.muted).nowrap());
        };
        // A swatch shows what the colour does: an "A" in it, or an "A" on
        // its wash, as the page would draw it.
        let (dark, page) = (l.c.dark, t.bg);
        let swatch = |ui: &mut Ui<'_>, i: u64, c: u32| {
            let name = crate::design::tint::name(c);
            let (fill, letter) = if ink {
                (t.hover.with_alpha(0.0), crate::design::tint::ink(c, dark))
            } else {
                (crate::design::tint::wash(c, dark, page), t.fg)
            };
            ui.with_indexed(
                i,
                NodeSpec::row()
                    .size(26.0, 26.0)
                    .radius(6.0)
                    .center()
                    .bg(fill)
                    .hover_bg(if ink { t.hover } else { fill })
                    .border(1.5, if c == now { t.accent } else if ink { Color::TRANSPARENT } else { t.border })
                    .on_click(Msg::Tint { ink: k, color: c as u64 })
                    .keep_focus()
                    .label(name.clone())
                    .tooltip(&name),
                |ui| {
                    ui.text_in(NodeSpec::row().role(Role::None), "A", TextStyle::new(15.0).family(l.serif).color(letter).nowrap());
                },
            );
        };
        let row = |ui: &mut Ui<'_>, key: &str, colors: &[u32], from: u64| {
            ui.with_keyed(key, NodeSpec::row().gap(3.0), |ui| {
                for (i, &c) in colors.iter().enumerate() {
                    swatch(ui, from + i as u64, c);
                }
            });
        };
        ui.with(
            NodeSpec::column()
                .float(FloatConfig::viewport().offset(at.x, at.y).fit())
                .pad(12.0)
                .gap(6.0)
                .radius(radius::CARD)
                .bg(l.card)
                .border(1.0, t.border)
                .shadow_color(l.shadow)
                .shadow_blur(18.0)
                .shadow_y(4.0)
                // Not modal: the toolbar's colour and highlighter buttons
                // stay in reach and switch it between the two, so a colour
                // and a highlight go on together. A press on it between
                // its swatches is its own; a press anywhere else, typing
                // or Esc put it away.
                .on_click(Value::Null)
                .keep_focus()
                .role(Role::Dialog)
                .label(if ink { "Text colour" } else { "Highlight" }),
            |ui| {
                // The two tabs.
                ui.with(
                    NodeSpec::row()
                        .grow_width()
                        .pad(2.0)
                        .gap(2.0)
                        .radius(radius::CONTROL)
                        .bg(t.sunken)
                        .role(Role::TabList)
                        .label("Colour for"),
                    |ui| {
                        for (key, name, which) in [("tab-ink", "Text colour", true), ("tab-mark", "Highlight", false)] {
                            let on = which == ink;
                            ui.with_keyed(
                                key,
                                NodeSpec::row()
                                    .grow_width()
                                    .pad_xy(10.0, 4.0)
                                    .radius(radius::CONTROL - 2.0)
                                    .center()
                                    .bg(if on { l.card } else { Color::TRANSPARENT })
                                    .hover_bg(if on { l.card } else { t.hover })
                                    .on_click(Msg::TintMenu { ink: which as u64 })
                                    .keep_focus()
                                    .role(Role::Tab)
                                    .checked(on)
                                    .label(name),
                                |ui| {
                                    ui.text(name, TextStyle::new(Type::SMALL.size).color(if on { t.fg } else { t.muted }).nowrap());
                                },
                            );
                        }
                    },
                );
                let hues: Vec<u32> = crate::design::tint::HUES
                    .iter()
                    .map(|h| if ink { (h.ink << 8) | 0xff } else { h.wash })
                    .collect();
                row(ui, "hues", &hues, 0);
                if !self.settings.colors.is_empty() {
                    label(ui, "Recent");
                    row(ui, "recent", &self.settings.colors, 200);
                }
                ui.with(NodeSpec::row().grow_width().gap(6.0).padding(Edges { l: 0.0, r: 0.0, t: 4.0, b: 0.0 }), |ui| {
                    let pill = |ui: &mut Ui<'_>, key: &str, name: &str, color: u32, dot: Option<Color>| {
                        ui.with_keyed(
                            key,
                            NodeSpec::row()
                                .pad_xy(10.0, 5.0)
                                .gap(6.0)
                                .radius(radius::CONTROL)
                                .border(1.0, t.border)
                                .hover_bg(t.hover)
                                .cross_align(Align::Center)
                                .on_click(Msg::Tint { ink: k, color: color as u64 })
                                .keep_focus()
                                .label(name),
                            |ui| {
                                match dot {
                                    Some(c) => {
                                        ui.leaf(NodeSpec::row().size(12.0, 12.0).radius(3.0).bg(c).role(Role::None));
                                    }
                                    None => crate::icons::icon(ui, Icon::Cross, t.muted, 12.0, false),
                                }
                                ui.text(name, TextStyle::new(Type::SMALL.size).color(t.fg).nowrap());
                            },
                        );
                    };
                    pill(ui, "none", "No colour", 0, None);
                    if !ink {
                        let wash = crate::design::tint::highlight(l.c.dark);
                        pill(ui, "plain", "Highlight", carnet_core::inline::HIGHLIGHT, Some(wash));
                    }
                });
            },
        );
    }

    /// The tools that did not fit, as a menu under the More button: each
    /// with its shortcut, ticked when on, greyed when it does nothing now.
    pub(super) fn tools_menu(&self) -> Vec<MenuItem> {
        let mut items = Vec::new();
        for (i, d) in self.overflow.iter().enumerate() {
            // The strip's groups, kept apart in the menu too.
            if i > 0 && self.overflow[i - 1].group != d.group {
                items.push(MenuItem::separator());
            }
            let mut item = MenuItem::new(d.name)
                .id(Value::map([("tool", Value::Int(i as i64))]))
                .enabled(d.enabled);
            if let Some(spec) = d.spec {
                item = item.accel(spec);
            }
            if d.toggle == Some(true) {
                item = item.checked(true);
            }
            items.push(item);
        }
        items
    }
}

/// A toolbar tool as data: drawn while its group fits, an item of the More
/// menu when it does not.
#[derive(Clone, Debug)]
pub(super) struct ToolDef {
    key: &'static str,
    icon: Icon,
    name: &'static str,
    hint: String,
    /// Its shortcut, for the menu.
    spec: Option<&'static str>,
    pub(super) msg: Msg,
    enabled: bool,
    toggle: Option<bool>,
    /// Which group it is in: what the More menu separates by.
    group: usize,
}
/// Toolbar icons are drawn outlines (crate::icons), never glyphs from a
/// font: the same on every machine.
pub(super) use crate::icons::Glyph as Icon;

/// A quiet toolbar button: an icon in `muted` that darkens under the
/// pointer, on the accent's wash while it names the active block's kind.
/// Its name contains what it shows ("H1 heading"), its tooltip says how to
/// type it, and a toggle tells a screen reader whether it is on.
pub(super) struct Tool<'a> {
    key: &'a str,
    icon: Icon,
    name: &'a str,
    hint: String,
    msg: Msg,
    enabled: bool,
    toggle: Option<bool>,
    filled: bool,
    /// Faded out while the reader writes.
    calm: bool,
}

impl<'a> Tool<'a> {
    pub(super) fn new(key: &'a str, icon: Icon, name: &'a str, hint: impl Into<String>, msg: Msg) -> Self {
        Self {
            key,
            icon,
            name,
            hint: hint.into(),
            msg,
            enabled: true,
            toggle: None,
            filled: false,
            calm: false,
        }
    }

    /// Out of sight while words are going down: it fades slowly, so the
    /// page is not seen to change, and is back at once when wanted.
    pub(super) fn calm(mut self, calm: bool) -> Self {
        self.calm = calm;
        self
    }

    /// The icon filled in, not only outlined (a pinned star).
    pub(super) fn filled(mut self, on: bool) -> Self {
        self.filled = on;
        self
    }

    pub(super) fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub(super) fn toggle(mut self, on: bool) -> Self {
        self.toggle = Some(on);
        self
    }

    pub(super) fn draw(self, ui: &mut Ui<'_>, t: &Theme) {
        let on = self.toggle == Some(true);
        let ink = if on { t.fg } else { t.muted };
        let mut spec = NodeSpec::row()
            .min_width(30.0)
            .height(30.0)
            .radius(radius::CONTROL)
            .center()
            // Off is the wash with no alpha, not clear black: an eased
            // colour goes from one to the other channel by channel, and
            // from black it would pass through a dark grey.
            .bg(if on { t.accent_soft } else { t.accent_soft.with_alpha(0.0) })
            .hover_bg(if on { t.accent_soft } else { t.hover })
            .pressed_bg(t.pressed)
            .on_click(self.msg)
            // A click leaves the keyboard where the writing is.
            .keep_focus()
            .disabled(!self.enabled)
            .opacity(match (self.calm, self.enabled) {
                (true, _) => 0.0,
                (false, true) => 1.0,
                (false, false) => crate::design::DISABLED_OPACITY,
            })
            .transition(if self.calm { 900.0 } else { 140.0 })
            .label(self.name)
            .tooltip(&self.hint)
            // Where it is: what a menu it opens hangs from.
            .on_layout(Value::Null);
        if let Some(state) = self.toggle {
            spec = spec.role(Role::Checkbox).checked(state);
        }
        let (icon, filled) = (self.icon, self.filled);
        let ink = if filled { t.accent } else { ink };
        ui.with_keyed(self.key, spec.width(30.0).center(), |ui| {
            crate::icons::icon(ui, icon, ink, 18.0, filled);
        });
    }
}
/// "just now", "5 minutes ago", "3 hours ago", "2 days ago".
pub(super) fn ago(now: SystemTime, then: SystemTime) -> String {
    let s = now.duration_since(then).unwrap_or_default().as_secs();
    let unit = |n: u64, one: &str| {
        if n == 1 {
            format!("1 {one} ago")
        } else {
            format!("{n} {one}s ago")
        }
    };
    match s {
        0..60 => "just now".into(),
        60..3600 => unit(s / 60, "minute"),
        3600..86_400 => unit(s / 3600, "hour"),
        86_400..1_209_600 => unit(s / 86_400, "day"),
        _ => unit(s / 604_800, "week"),
    }
}

/// A day as people write it: "12 January 2027" (UTC).
pub(super) fn date_words(t: SystemTime) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let days = (t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs() / 86_400) as i64;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

/// The library's terse form: "now", "5m", "3h", "2d", "6w".
pub(super) fn short_ago(now: SystemTime, then: SystemTime) -> String {
    let s = now.duration_since(then).unwrap_or_default().as_secs();
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        86_400..1_209_600 => format!("{}d", s / 86_400),
        _ => format!("{}w", s / 604_800),
    }
}

