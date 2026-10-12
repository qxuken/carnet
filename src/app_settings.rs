//! The settings, shown in place of the note: how the page looks, how the
//! library is ordered, where the notes live, and sync. Every change applies at
//! once and is written straight away; there is no Save.

use std::path::PathBuf;

use kui_native::{FontFamily, Span};

use super::chrome::{Icon, Tool};
use super::*;
use carnet_sync::compat::feature;
use crate::design;
use crate::look;

/// Settings → Sync's field maker: a field by its key, its text, its hint,
/// and what it holds.
type MakeField<'a> = dyn Fn(&mut Carnet, &mut Ui<'_>, &str, &str, &str, Field) -> Key + 'a;

/// One row of the font list.
const FONT_ROW_H: f32 = 34.0;

impl Carnet {
    pub(super) fn on_settings(&mut self, msg: Msg) {
        // The column's switch with a note's history in it: the history
        // goes, and the library with it if it showed.
        if matches!(msg, Msg::ToggleSidebar) && self.versions.is_some() {
            self.close_history();
            if !self.side_shown() {
                return;
            }
        }
        let s = &mut self.settings;
        match msg {
            Msg::ShowSettings => {
                self.settings_open = !self.settings_open;
                self.wait_line = Default::default();
                self.font_pick = None;
                self.list_view = false;
                return;
            }
            Msg::SetTheme { to } => {
                s.theme = match to {
                    1 => ThemePref::Light,
                    2 => ThemePref::Dark,
                    _ => ThemePref::System,
                }
            }
            Msg::SetPalette { to } => {
                s.palette = design::Palette::ALL[(to as usize).min(design::Palette::ALL.len() - 1)];
            }
            Msg::SetDensity { to } => {
                s.density = match to {
                    0 => design::Density::Compact,
                    2 => design::Density::Airy,
                    _ => design::Density::Comfortable,
                };
            }
            Msg::SetPairing { to } => {
                let (_, text, heading) = design::PAIRINGS[(to as usize).min(design::PAIRINGS.len() - 1)];
                s.font = text.to_string();
                s.heading_font = heading.to_string();
                self.font_pick = None;
            }
            Msg::SetWidth { to } => {
                s.width = match to {
                    0 => Width::Narrow,
                    2 => Width::Wide,
                    _ => Width::Medium,
                }
            }
            Msg::SetWindow { to } => s.translucent = to == 1,
            Msg::SetToolbar { to } => s.fade_tools = to == 1,
            Msg::SetFocus { to } => s.focus = to == 1,
            Msg::SetTypewriter { to } => s.typewriter = to == 1,
            // From zen, the library's switch shows it: zen is over.
            Msg::ToggleSidebar if self.zen => {
                self.zen = false;
                s.sidebar = true;
            }
            Msg::ToggleSidebar => s.sidebar = !s.sidebar,
            Msg::SetSort { to } => {
                s.sort = match to {
                    1 => Sort::Title,
                    2 => Sort::Created,
                    _ => Sort::Edited,
                };
                self.reorder();
            }
            Msg::SetDates { to } => s.dates_created = to == 1,
            // Asked first: everything on the Appearance and Writing cards
            // goes.
            Msg::ResetLook if !self.reset_armed => {
                self.reset_armed = true;
                return;
            }
            Msg::ResetLook => {
                self.reset_armed = false;
                s.reset_look();
                self.font_pick = None;
                self.notify("Appearance and writing reset", None);
            }
            Msg::ResetLookCancel => {
                self.reset_armed = false;
                return;
            }
            Msg::SetSize { by } => {
                let (lo, hi) = (*crate::settings::SIZES.start() as i64, *crate::settings::SIZES.end() as i64);
                s.size = (s.size as i64 + by).clamp(lo, hi) as u32;
            }
            Msg::SetDivider { to } => {
                s.divider = crate::settings::Divider::ALL[(to as usize).min(crate::settings::Divider::ALL.len() - 1)];
            }
            Msg::SetInitialLines { to } => {
                s.initial_lines = (to as u32 + 2).clamp(*crate::settings::INITIAL_LINES.start(), *crate::settings::INITIAL_LINES.end());
            }
            Msg::SetInitialColor { to } => s.initial_accent = to == 0,
            // Each time afresh: the whole list, the search empty.
            Msg::FontMenu { which } => {
                self.font_pick = if self.font_pick == Some(which) { None } else { Some(which) };
                self.font_query.clear();
                self.font_search_reset = true;
                return;
            }
            Msg::SetFont { which, index } => {
                let name = if index == ALL {
                    String::new()
                } else {
                    match self.font_rows.get(index as usize) {
                        Some(n) => n.clone(),
                        None => return,
                    }
                };
                match which {
                    0 => s.font = name,
                    1 => s.heading_font = name,
                    _ => s.initial_font = name,
                }
                self.font_pick = None;
            }
            Msg::PickDir => {
                self.pick_dir = true;
                return;
            }
            Msg::ResetDir => {
                self.dir_change = Some(None);
                return;
            }
            Msg::ShowDir => {
                if let Err(e) = store::reveal(&self.store.dir) {
                    self.error = Some(format!("Couldn't open the folder: {e}"));
                }
                return;
            }
            _ => return,
        }
        self.save_settings();
    }

    /// Settings → Sync's way in, its turn: the server's answer to what it
    /// is taken (fine: on to how you're getting in, or straight to the
    /// code with nothing to choose between); an invite sent, its code to
    /// be typed where codes go. Once in, the steps start again for next
    /// time.
    pub(super) fn connect_tick(&mut self) {
        if self.sync.is_on() && self.connect.step() != crate::connect::Step::Server {
            self.connect.change();
            self.signup.reset();
        }
        if self.connect.tick() {
            self.sync_how = 0;
            self.signup.reset();
            self.sync.error = None;
            self.sync_reset_fields = true;
            if self.connect.step() == crate::connect::Step::Form {
                self.focus_req = Some(Field::SyncCode);
            }
        }
        if self.signup.tick() {
            self.sync_how = 0;
            self.sync_code.clear();
            self.sync_reset_fields = true;
            self.sync.error = None;
            self.focus_req = Some(Field::SyncCode);
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.error = Some(format!("Couldn't save the settings: {e}"));
        }
    }

    /// Enter in the font list's search: the first match; the built-in
    /// face when it is the only one.
    pub(super) fn apply_typed_font(&mut self) {
        let Some(which) = self.font_pick else { return };
        let q = self.font_query.trim().to_lowercase();
        if !self.font_rows.is_empty() {
            self.on_settings(Msg::SetFont { which, index: 0 });
        } else if built_in(which).is_some_and(|f| f.to_lowercase().contains(&q)) {
            self.on_settings(Msg::SetFont { which, index: ALL });
        }
    }

    /// Moves to another notes folder (`None`: `~/Documents/Carnet`):
    /// everything here saved first, then the app starts over on the new
    /// folder, settings open where they were.
    pub(super) fn change_dir(&mut self, dir: Option<PathBuf>, core: &mut Core) {
        let target = dir.clone().unwrap_or_else(store::documents_dir);
        if same_dir(&target, &self.store.dir) {
            self.settings.dir = dir;
            self.save_settings();
            return;
        }
        self.title_focused = false;
        self.save_current();
        if let Err(e) = self.store.save_all() {
            self.error = Some(format!("Couldn't save before switching folders: {e}"));
            return;
        }
        match Store::open(target.clone()) {
            Ok(store) => {
                self.settings.dir = dir;
                let mut next = Carnet::with_settings(store, self.settings.clone());
                self.images.clear(core);
                next.images = std::mem::take(&mut self.images);
                next.styler = std::mem::take(&mut self.styler);
                next.families = self.families.take();
                next.font_faces = std::mem::take(&mut self.font_faces);
                next.settings_open = true;
                *self = next;
                self.save_settings();
                self.notify(format!("Notes now live in {}", target.display()), None);
            }
            Err(e) => self.error = Some(format!("Couldn't open {}: {e}", target.display())),
        }
    }

    /// A face by name, for drawing a font's name in itself.
    fn face_of(faces: &mut HashMap<String, FontFamily>, ui: &mut Ui<'_>, name: &str) -> FontFamily {
        *faces
            .entry(name.to_string())
            .or_insert_with(|| ui.system_font(name).map_or(FontFamily::Sans, FontFamily::Custom))
    }

    pub(super) fn settings_page(&mut self, ui: &mut Ui<'_>, l: &Look, narrow: bool, main_w: f32) {
        let t = l.t;
        // The strip: the way back, the window's buttons.
        ui.with(
            NodeSpec::row()
                .grow_width()
                .height(STRIP_H)
                .cross_align(Align::Center)
                .window_drag_unless(self.menu_open),
            |ui| {
                // The leftmost strip only when the library is not beside it.
                gap(ui, if narrow || !self.settings.sidebar { 8.0_f32.max(self.lights) } else { 8.0 });
                Tool::new("t-close-settings", Icon::Back, "Back to the note", "Back  \u{b7}  Esc".to_string(), Msg::ShowSettings)
                    .draw(ui, &t);
                ui.leaf(NodeSpec::row().grow_width());
                widgets::window_buttons(ui);
            },
        );
        let pad = if narrow || main_w < 640.0 { 18.0 } else { 48.0 };
        ui.with_keyed(
            "settings",
            NodeSpec::column()
                .grow_width()
                .grow_height()
                .scroll_y()
                .scrollbar_width(5.0)
                .cross_align(Align::Center),
            |ui| {
                ui.with(
                    NodeSpec::column()
                        .grow_width()
                        // As wide as a note: the page width chosen here
                        // shows here too.
                        .max_width(l.page_w)
                        .padding(Edges { l: pad, r: pad, t: 18.0, b: 80.0 })
                        .gap(10.0),
                    |ui| self.settings_body(ui, l),
                );
            },
        );
        let _ = t;
    }

    fn settings_body(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let t = l.t;
        // Five quick clicks on it turn the debug information on or off:
        // hidden, for a release build.
        ui.with_keyed("settings-title", NodeSpec::row().on_click(Msg::SettingsTitle).role(Role::Heading), |ui| {
            ui.text(
                "Settings",
                TextStyle::new(Type::DISPLAY.size).line_height(Type::DISPLAY.line).family(l.serif).color(t.fg),
            );
        });
        if let Some(e) = &self.error {
            ui.text_in(
                NodeSpec::row().live(kui_native::Live::Assertive).selectable(),
                e,
                TextStyle::new(Type::UI.size).color(t.danger),
            );
        } else if let Some(n) = &self.notice {
            ui.text_in(NodeSpec::row().selectable(), &n.text, TextStyle::new(Type::UI.size).color(t.muted));
        }

        // -- how it looks
        section(ui, l, "Appearance");
        let s = self.settings.clone();
        card(ui, l, "card-look", |ui| {
            // The palettes, each a swatch in the mode now showing.
            setting(ui, l, "Colors", None, |ui| {
                ui.with_keyed(
                    "palettes",
                    NodeSpec::row().gap(design::space::S).wrap().role(Role::Group).label("Colors"),
                    |ui| {
                        for (i, p) in design::Palette::ALL.iter().enumerate() {
                            swatch(ui, l, i, *p, s.palette == *p);
                        }
                    },
                );
            });
            rule(ui, l);
            setting(ui, l, "Theme", None, |ui| {
                let cur = match s.theme {
                    ThemePref::System => 0,
                    ThemePref::Light => 1,
                    ThemePref::Dark => 2,
                };
                segmented(ui, l, "theme", &["System", "Light", "Dark"], cur, |to| Msg::SetTheme { to });
            });
            rule(ui, l);
            setting(ui, l, "Page width", None, |ui| {
                let cur = match s.width {
                    Width::Narrow => 0,
                    Width::Medium => 1,
                    Width::Wide => 2,
                };
                segmented(ui, l, "width", &["Narrow", "Medium", "Wide"], cur, |to| Msg::SetWidth { to });
            });
            rule(ui, l);
            setting(ui, l, "Density", Some("Space between blocks and notes"), |ui| {
                let cur = match s.density {
                    design::Density::Compact => 0,
                    design::Density::Comfortable => 1,
                    design::Density::Airy => 2,
                };
                segmented(ui, l, "density", &["Compact", "Comfortable", "Airy"], cur, |to| Msg::SetDensity { to });
            });
            rule(ui, l);
            setting(ui, l, "Window", Some("The desktop through the library, blurred; from the next start"), |ui| {
                let cur = if s.translucent { 1 } else { 0 };
                segmented(ui, l, "window", &["Solid", "Translucent"], cur, |to| Msg::SetWindow { to });
            });
            rule(ui, l);

            setting(ui, l, "Text size", None, |ui| {
                ui.with(
                    NodeSpec::row().pad(3.0).gap(2.0).radius(radius::CONTROL).bg(t.sunken).cross_align(Align::Center),
                    |ui| {
                        let lo = s.size <= *crate::settings::SIZES.start();
                        let hi = s.size >= *crate::settings::SIZES.end();
                        step_button(ui, l, "size-down", Icon::Minus, "Smaller text", !lo, Msg::SetSize { by: -1 });
                        ui.text_in(
                            NodeSpec::row().width(54.0).center(),
                            &format!("{} px", s.size),
                            TextStyle::new(Type::UI.size).color(t.fg).nowrap(),
                        );
                        step_button(ui, l, "size-up", Icon::Plus, "Bigger text", !hi, Msg::SetSize { by: 1 });
                    },
                );
            });
            rule(ui, l);
            setting(ui, l, "Dividers", Some("How a divider looks on the page"), |ui| {
                ui.with_keyed(
                    "dividers",
                    NodeSpec::row().gap(design::space::S).wrap().role(Role::Group).label("Dividers"),
                    |ui| {
                        for (i, d) in crate::settings::Divider::ALL.iter().enumerate() {
                            divider_choice(ui, l, i, *d, s.divider == *d);
                        }
                    },
                );
            });
            rule(ui, l);
            // A letter drops only where its paragraph asks, from the
            // toolbar; these style every one of them.
            let how = format!("Drop a paragraph's first letter from the toolbar, or {}", crate::keys::label(crate::keys::INITIAL));
            setting(ui, l, "Dropped letters", Some(&how), |_| {});
            subgroup(ui, l, "initial-styles", "Dropped letters", |ui| {
                sub_setting(ui, l, "Up to", |ui| {
                    let cur = (s.initial_lines - crate::settings::INITIAL_LINES.start()) as usize;
                    segmented(ui, l, "height", &["2 lines", "3 lines", "4 lines"], cur, |to| Msg::SetInitialLines { to });
                });
                let name = if s.initial_font.is_empty() { default_font_name(2).to_string() } else { s.initial_font.clone() };
                sub_setting(ui, l, "Font", |ui| {
                    font_pill(ui, l, "font-initial", &name, l.initial, self.font_pick == Some(2), Msg::FontMenu { which: 2 });
                });
                if self.font_pick == Some(2) {
                    self.font_list(ui, l, 2, &s.initial_font);
                }
                sub_setting(ui, l, "Colour", |ui| {
                    let cur = if s.initial_accent { 0 } else { 1 };
                    segmented(ui, l, "colour", &["Accent", "Text"], cur, |to| Msg::SetInitialColor { to });
                });
            });
            rule(ui, l);
            setting(ui, l, "Fonts", Some("A pairing, or choose each below"), |ui| {
                let names: Vec<&str> = design::PAIRINGS.iter().map(|p| p.0).collect();
                let cur = design::PAIRINGS
                    .iter()
                    .position(|(_, t, h)| *t == s.font && *h == s.heading_font)
                    .unwrap_or(usize::MAX);
                segmented(ui, l, "pairing", &names, cur, |to| Msg::SetPairing { to });
            });
            // Each face of the pairing, chosen on its own.
            subgroup(ui, l, "faces", "Each font", |ui| {
                let body_name = if is_built_in(0, &s.font) { default_font_name(0).to_string() } else { s.font.clone() };
                sub_setting(ui, l, "Text", |ui| {
                    font_pill(ui, l, "font-body", &body_name, l.body, self.font_pick == Some(0), Msg::FontMenu { which: 0 });
                });
                if self.font_pick == Some(0) {
                    self.font_list(ui, l, 0, &s.font);
                }
                let head_name = if is_built_in(1, &s.heading_font) { default_font_name(1).to_string() } else { s.heading_font.clone() };
                sub_setting(ui, l, "Headings", |ui| {
                    font_pill(ui, l, "font-head", &head_name, l.serif, self.font_pick == Some(1), Msg::FontMenu { which: 1 });
                });
                if self.font_pick == Some(1) {
                    self.font_list(ui, l, 1, &s.heading_font);
                }
            });
        });
        // What a note will look like, at the size and in the faces chosen.
        ui.with(
            NodeSpec::column()
                .grow_width()
                .pad_xy(22.0, 18.0)
                .gap(6.0)
                .radius(radius::CARD)
                .border(1.0, t.border)
                .selectable()
                .label("Preview"),
            |ui| {
                ui.text(
                    "A calm place for notes",
                    l.page_type(Type::H2).family(l.serif).color(t.fg),
                );
                // Long enough to wrap past a dropped letter four lines
                // tall, at the widest page.
                let body = l.page_type(Type::BODY).family(l.body).color(t.fg);
                let words = "Lists, tables and pictures, kept as plain Markdown files you own. \
                             Nothing moves unless you move it. Write a line, a page or a book; \
                             colour a word, drop a paragraph's first letter, pin the notes you come back to, \
                             and find any of them again by a word in it. Every note is a file in \
                             your folder, readable by any other app, today and in twenty years, \
                             whatever happens to this one.";
                // A first letter dropped, in its face and ink, as any
                // paragraph's from the toolbar.
                let (cap, rest) = words.split_at(1);
                let ink = if s.initial_accent { l.c.accent_ink } else { t.fg };
                ui.rich_text(
                    &[Span::new(cap).family(l.initial).color(ink), Span::new(rest)],
                    body.initial_letter(s.initial_lines),
                );
            },
        );

        // -- how the page behaves while words are going down
        section(ui, l, "Writing");
        card(ui, l, "card-writing", |ui| {
            setting(ui, l, "Toolbar", Some("Back as soon as you reach for the pointer"), |ui| {
                let cur = if s.fade_tools { 1 } else { 0 };
                segmented(ui, l, "toolbar", &["Always shown", "Fades while typing"], cur, |to| Msg::SetToolbar { to });
            });
            rule(ui, l);
            setting(ui, l, "Focus", Some("While you type, the rest of the note steps back"), |ui| {
                let cur = if s.focus { 1 } else { 0 };
                segmented(ui, l, "focus", &["Off", "On"], cur, |to| Msg::SetFocus { to });
            });
            rule(ui, l);
            setting(ui, l, "Typewriter", Some("The line you write stays near the middle of the window"), |ui| {
                let cur = if s.typewriter { 1 } else { 0 };
                segmented(ui, l, "typewriter", &["Off", "On"], cur, |to| Msg::SetTypewriter { to });
            });
        });
        // Both cards above as they first were; asked first.
        let armed = self.reset_armed;
        card(ui, l, "card-reset", |ui| {
            setting(
                ui,
                l,
                "Reset appearance and writing",
                Some("Colors, fonts, sizes and the writing options, back as they were at first. Notes, the library, the window and sync stay as they are."),
                |ui| {
                    if armed {
                        pill_button(ui, l, "reset-cancel", "Cancel", false, true, Msg::ResetLookCancel);
                        pill_button(ui, l, "reset-yes", "Yes, reset", true, true, Msg::ResetLook);
                    } else {
                        pill_button(ui, l, "reset", "Reset", false, !s.look_is_default(), Msg::ResetLook);
                    }
                },
            );
        });

        // -- the library
        section(ui, l, "Library");
        card(ui, l, "card-library", |ui| {
            setting(ui, l, "Sort notes by", Some("Pinned notes stay on top"), |ui| {
                let cur = match s.sort {
                    Sort::Edited => 0,
                    Sort::Title => 1,
                    Sort::Created => 2,
                };
                segmented(ui, l, "sort", &["Last edited", "Title", "Created"], cur, |to| Msg::SetSort { to });
            });
            rule(ui, l);
            setting(ui, l, "Dates", Some("The date on each note, in the list and over its title"), |ui| {
                let cur = if s.dates_created { 1 } else { 0 };
                segmented(ui, l, "dates", &["Last edited", "Created"], cur, |to| Msg::SetDates { to });
            });
        });

        // -- every shortcut, as this platform writes it
        section(ui, l, "Keyboard");
        card(ui, l, "card-keys", |ui| {
            ui.with(
                NodeSpec::column().grow_width().pad_xy(18.0, 10.0).selectable(),
                |ui| {
                    for (i, (what, spec)) in crate::keys::ALL.iter().enumerate() {
                        ui.with_indexed(
                            i as u64,
                            NodeSpec::row().grow_width().height(28.0).cross_align(Align::Center),
                            |ui| {
                                ui.text_in(
                                    NodeSpec::row().grow_width().min_width(0.0),
                                    what,
                                    TextStyle::new(Type::UI.size).color(t.fg).nowrap(),
                                );
                                ui.text_in(
                                    NodeSpec::row().pad_xy(8.0, 2.0).radius(radius::TIGHT).bg(t.sunken),
                                    &crate::keys::label(spec),
                                    TextStyle::new(Type::SMALL.size).color(t.muted).nowrap(),
                                );
                            },
                        );
                    }
                },
            );
        });

        self.sync_settings(ui, l);

        // -- where the notes live
        section(ui, l, "Storage");
        let dir = self.store.dir.display().to_string();
        let custom = self.settings.dir.is_some();
        card(ui, l, "card-storage", |ui| {
            ui.with(
                NodeSpec::column().grow_width().pad_xy(18.0, 14.0).gap(4.0).selectable(),
                |ui| {
                    ui.text("Notes folder", TextStyle::new(Type::STRONG.size).color(t.fg));
                    ui.text(&dir, TextStyle::new(Type::SMALL.size).color(t.muted));
                    if std::env::var_os("CARNET_DIR").is_some() {
                        ui.text(
                            "CARNET_DIR is set: it chooses the folder when Carnet starts.",
                            TextStyle::new(Type::SMALL.size).color(t.muted),
                        );
                    }
                },
            );
            ui.with(
                NodeSpec::row()
                    .grow_width()
                    .padding(Edges { l: 14.0, r: 14.0, t: 0.0, b: 14.0 })
                    .gap(6.0)
                    .wrap(),
                |ui| {
                    pill_button(ui, l, "dir-change", "Change\u{2026}", true, true, Msg::PickDir);
                    pill_button(ui, l, "dir-show", reveal_label(), false, true, Msg::ShowDir);
                    pill_button(ui, l, "dir-copy", "Copy path", false, true, Msg::CopyText { what: 0 });
                    pill_button(ui, l, "dir-reset", "Use Documents/Carnet", false, custom, Msg::ResetDir);
                },
            );
        });
        ui.text_in(
            NodeSpec::row().pad_xy(4.0, 0.0).selectable(),
            "Every note is a Markdown file in this folder; folders in it are folders here. \
             Pasted and dropped pictures are kept in its attachments folder.",
            TextStyle::new(Type::SMALL.size).color(t.muted),
        );
        if let Some(file) = &self.settings.file {
            ui.text_in(
                NodeSpec::row().pad_xy(4.0, 0.0).selectable(),
                &format!("Settings are kept in {}", file.display()),
                TextStyle::new(Type::SMALL.size).color(t.muted),
            );
        }
    }

    /// Settings → Sync (docs/SYNC.md §5, §7): off, the way in chosen and
    /// tried; on, what it is doing, a device to let in, the recovery
    /// words. What goes wrong is said here, in place, never in a box.
    /// The inspector (docs/SYNC.md §9): each note the server holds that is
    /// worth a look, said in plain words, an icon to each line; and what
    /// was done lately. A note's ways to fix it are behind its More, each
    /// with what it does, and asked before it's done. A note in step, kept
    /// as plain saves, is only counted.
    fn sync_inspector_view(&mut self, ui: &mut Ui<'_>, l: &Look) {
        use crate::sync::Status;
        let t = l.t;
        let small = |c| TextStyle::new(Type::SMALL.size).color(c);
        let Some((rooms, log)) = self.sync.inspection.clone() else {
            ui.text_in(NodeSpec::row().pad_xy(18.0, 10.0), "Asking the server\u{2026}", small(t.muted));
            return;
        };
        let online = self.sync.status == Status::Online;
        let now = SystemTime::now();
        let secs = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let ago = |s: u64| super::chrome::ago(now, SystemTime::UNIX_EPOCH + Duration::from_secs(s));
        let names: HashMap<Vec<u8>, String> = self
            .sync
            .devices
            .iter()
            .flatten()
            .map(|d| (d.id.clone(), if d.you { "this device".into() } else { d.name.clone().unwrap_or_else(|| "another device".into()) }))
            .collect();
        let name_of = |id: &[u8]| names.get(id).cloned().unwrap_or_else(|| "a device no longer in the account".into());
        // The rooms this device has a copy of: every note's, those in
        // Deleted too, and the vault's (where the notes go).
        let have: std::collections::HashSet<String> = self.store.replica.as_ref().map(|r| carnet_sync::session::Docs::rooms(r).into_iter().collect()).unwrap_or_default();
        let title_of = |me: &Self, room: &str| sync_room_title(&me.store, room);
        let saves = |n: u64| if n == 1 { "1 save".to_string() } else { format!("{n} saves") };
        let edits = |n: u64| if n == 1 { "1 edit".to_string() } else { format!("{n} edits") };
        // A line: an icon, then words.
        let line = |ui: &mut Ui<'_>, g: Icon, ink: Color, words: &str, style: TextStyle| {
            ui.with(NodeSpec::row().grow_width().gap(8.0).cross_align(Align::Center), |ui| {
                crate::icons::icon(ui, g, ink, 15.0, false);
                ui.text_in(NodeSpec::row().grow_width(), words, style);
            });
        };
        // When the server said this, and whether the notes have changed
        // since (it's asked again once they're quiet).
        let checked = match (self.sync.inspection_stale(), self.sync.inspected_at) {
            (true, _) => "Changed since: checking again\u{2026}".to_string(),
            (false, Some(at)) => format!("Checked {}", super::chrome::ago(now, at)),
            (false, None) => String::new(),
        };
        if !checked.is_empty() {
            ui.with(NodeSpec::row().grow_width().padding(Edges { l: 18.0, r: 18.0, t: 8.0, b: 2.0 }), |ui| line(ui, Icon::Clock, t.muted, &checked, small(t.muted)));
        }
        // Notes the server has and this device hasn't: said first, in so
        // many words, never folded into "up to date".
        let here_as_note = |me: &Self, room: &str| {
            room == carnet_sync::vault::ROOM
                || carnet_sync::Key::parse(room).is_some_and(|k| me.store.notes.iter().any(|n| n.key == Some(k)) || me.store.replica.as_ref().is_some_and(|r| r.entry(k).is_some()))
        };
        let lacking = rooms.iter().filter(|r| !r.held.is_empty() && !here_as_note(self, &r.room)).count();
        if lacking > 0 {
            let words = match lacking {
                1 => "1 note on the server isn't on this device yet.".to_string(),
                n => format!("{n} notes on the server aren't on this device yet."),
            };
            ui.with(NodeSpec::row().grow_width().pad_xy(18.0, 10.0), |ui| line(ui, Icon::Down, t.warning, &words, small(t.fg)));
        }
        let (mut quiet, mut shown) = (0, 0);
        for (i, r) in rooms.iter().enumerate() {
            // This device's copy against the server's, by peer. One this
            // device has no copy of has nothing here to compare, nor to
            // replace the server's with.
            let mine_here = have.contains(&r.room);
            let here = match (mine_here, self.store.replica.as_mut()) {
                (true, Some(rep)) => carnet_sync::session::Docs::doc(rep, &r.room).version_vector(),
                _ => Vec::new(),
            };
            let ahead = |a: &[(u64, u64)], b: &[(u64, u64)]| -> u64 {
                let b: HashMap<u64, u64> = b.iter().copied().collect();
                a.iter().map(|(p, c)| c.saturating_sub(b.get(p).copied().unwrap_or(0))).sum()
            };
            let (mine, theirs) = (ahead(&here, &r.held), ahead(&r.held, &here));
            if r.base.is_none() && r.shelved == 0 && mine == 0 && theirs == 0 {
                // The vault in step isn't a note: not counted as one.
                if r.room != carnet_sync::vault::ROOM {
                    quiet += 1;
                }
                continue;
            }
            shown += 1;
            let title = title_of(self, &r.room);
            let (g, ink, state) = match (mine, theirs) {
                (0, 0) => (Icon::Check, t.success, "Up to date".to_string()),
                (m, 0) => (Icon::Upload, t.muted, format!("{} on this device, not on the server yet", edits(m))),
                (0, s) => (Icon::Down, t.muted, format!("{} on the server, not on this device yet", edits(s))),
                (m, s) => (Icon::Warning, t.warning, format!("Out of step: {} to send, {} to get", edits(m), edits(s))),
            };
            let open = self.sync_inspect_open.as_deref() == Some(r.room.as_str());
            let armed = self.sync_inspect_armed.as_ref().filter(|(room, _)| *room == r.room).map(|(_, repair)| *repair);
            let days_left = (r.shelved_since + KEEP_DAYS * 86_400).saturating_sub(secs).div_ceil(86_400).max(1);
            let undoable = r.base.as_ref().is_some_and(|b| b.undoable) && r.shelved > 0;
            // Its ways to fix it: undo packing, if it's packed; replace the
            // server's copy with this device's, if this device has one.
            let ways = r.base.is_some() || mine_here;
            ui.with_key(ui.child_key("sync-room").index(i as u64), NodeSpec::column().grow_width().padding(Edges { l: 18.0, r: 18.0, t: 10.0, b: 10.0 }).gap(6.0), |ui| {
                // The note, and More.
                ui.with(NodeSpec::row().grow_width().gap(10.0).cross_align(Align::Center), |ui| {
                    ui.text_in(NodeSpec::row().grow_width(), &title, TextStyle::new(Type::STRONG.size).color(t.fg));
                    if ways {
                        let word = if open { "Hide ways to fix" } else { "Ways to fix" };
                        icon_pill(ui, l, &format!("sync-more-{i}"), Icon::More, word, false, true, Msg::SyncInspectMore { at: i as u64 });
                    }
                });
                line(ui, g, ink, &state, small(t.fg));
                match &r.base {
                    Some(b) => {
                        let since = if r.live == 0 { String::new() } else { format!(", and {} since", saves(r.live)) };
                        line(ui, Icon::Layers, t.muted, &format!("Packed into one copy {} by {}{since}", ago(b.at), name_of(&b.by)), small(t.muted));
                        if b.verified {
                            line(ui, Icon::Check, t.muted, "Checked by another device", small(t.muted));
                        } else {
                            line(ui, Icon::Clock, t.muted, "Not checked by another device yet", small(t.muted));
                        }
                    }
                    None => line(ui, Icon::Layers, t.muted, &format!("Kept as {}", saves(r.live)), small(t.muted)),
                }
                if !open || !ways {
                    return;
                }
                // The ways to fix it, each with what it does.
                // Each in an outlined box, so its button (on the sunken well,
                // as every button here) reads as one; the button says the
                // verb, the box what it does.
                let way = |ui: &mut Ui<'_>, key: &str, g: Icon, name: &str, verb: &str, what: &str, enabled: bool, msg: Msg, repair: bool| {
                    ui.with(
                        NodeSpec::row()
                            .grow_width()
                            .gap(12.0)
                            .padding(Edges { l: 12.0, r: 12.0, t: 10.0, b: 10.0 })
                            .radius(radius::CONTROL)
                            .border(1.0, if armed == Some(repair) { t.border_strong } else { t.border })
                            .cross_align(Align::Center)
                            .wrap(),
                        |ui| {
                            crate::icons::icon(ui, g, t.fg, 18.0, false);
                            ui.with(NodeSpec::column().grow_width().min_width(200.0).gap(2.0), |ui| {
                                ui.text(name, TextStyle::new(Type::UI.size).color(t.fg));
                                ui.text(what, small(t.muted));
                            });
                            if armed == Some(repair) {
                                pill_button(ui, l, &format!("{key}-cancel"), "Cancel", false, true, Msg::SyncInspectCancel);
                                pill_button(ui, l, &format!("{key}-yes"), &format!("Yes, {}", verb.to_lowercase()), true, online, Msg::SyncInspectConfirm);
                            } else {
                                pill_button(ui, l, key, verb, false, enabled && online, msg);
                            }
                        },
                    );
                };
                if r.base.is_some() {
                    let what = if undoable {
                        format!(
                            "Bring back the {} it was packed from, as they were. Possible for {} more {}.",
                            saves(r.shelved),
                            days_left,
                            if days_left == 1 { "day" } else { "days" }
                        )
                    } else {
                        "No longer possible: the saves it was packed from are kept for 30 days, and that has passed.".into()
                    };
                    way(ui, &format!("sync-undo-{i}"), Icon::Undo, "Undo packing", "Undo", &what, undoable, Msg::SyncRollBack { at: i as u64 }, false);
                }
                if mine_here {
                    let what = if r.room == carnet_sync::vault::ROOM {
                        "Make the server's folders and note names the same as this device's. For when notes are in the wrong place on your other devices."
                    } else {
                        "Make the server's copy of this note the same as this device's. For when the note looks wrong on your other devices."
                    };
                    way(ui, &format!("sync-replace-{i}"), Icon::Upload, "Replace with this device's copy", "Replace", what, true, Msg::SyncRepair { at: i as u64 }, true);
                }
            });
        }
        if quiet > 0 || rooms.iter().all(|r| r.room == carnet_sync::vault::ROOM) {
            let more = if shown > 0 { " other" } else { "" };
            let words = match quiet {
                0 => "The server has no notes yet.".to_string(),
                1 => format!("1{more} note is up to date."),
                n => format!("{n}{more} notes are up to date."),
            };
            ui.with(NodeSpec::row().grow_width().pad_xy(18.0, 10.0), |ui| line(ui, Icon::Check, t.muted, &words, small(t.muted)));
        }
        if !log.is_empty() {
            ui.text_in(NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 8.0, b: 2.0 }), "Lately", small(t.muted));
            for (i, e) in log.iter().take(10).enumerate() {
                let (g, warn, what) = lately_words(&self.store, &e.room, &e.what);
                let ink = if warn { t.warning } else { t.fg };
                let words = format!("{what} \u{b7} {} \u{b7} {}", ago(e.at), name_of(&e.device));
                ui.with_key(ui.child_key("sync-log").index(i as u64), NodeSpec::row().grow_width().pad_xy(18.0, 3.0).selectable(), |ui| line(ui, g, ink, &words, small(t.fg)));
            }
        }
    }

    /// Settings → Sync while sync is off: the way in, a step at a time.
    /// The server first, asked what it is; then how you're getting in,
    /// from the ways it lists (none to choose between: skipped); then that
    /// way's form. `field` makes a field, as the card's others.
    fn sync_way_in(&mut self, ui: &mut Ui<'_>, l: &Look, starting: bool, field: &MakeField<'_>) {
        use crate::connect::Step;
        let t = l.t;
        let reset = std::mem::take(&mut self.sync_reset_fields);
        let step = self.connect.step();
        let choice = self.connect.choice();
        // A line said: an icon, then words.
        let said = |ui: &mut Ui<'_>, key: &str, g: Icon, ink: Color, words: &str| {
            ui.with_keyed(
                key,
                NodeSpec::row().grow_width().pad_xy(18.0, 2.0).gap(8.0).cross_align(Align::Center).selectable().live(kui_native::Live::Polite),
                |ui| {
                    crate::icons::icon(ui, g, ink, 15.0, false);
                    ui.text_in(NodeSpec::row().grow_width().min_width(0.0), words, TextStyle::new(Type::UI.size).color(ink));
                },
            );
        };
        // Where in the way in: "Step 2 of 3". With nothing to choose
        // between, there are two.
        let (n, of) = match step {
            Step::Server => (1, 3),
            Step::Way => (2, 3),
            Step::Form if choice => (3, 3),
            Step::Form => (2, 2),
        };
        ui.text_in(
            NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 14.0, b: 0.0 }),
            &format!("Step {n} of {of}"),
            TextStyle::new(Type::SMALL.size).color(t.muted).nowrap(),
        );
        // A row of a field and its buttons.
        let row = NodeSpec::row().grow_width().padding(Edges { l: 18.0, r: 18.0, t: 4.0, b: 14.0 }).gap(10.0).cross_gap(10.0).cross_align(Align::Center).wrap();
        match step {
            Step::Server => {
                // (A word on the hosted service goes here, once there's one.)
                setting(ui, l, "Cloud sync setup", Some("Enter the server URL."), |_| {});
                let checking = self.connect.checking();
                ui.with(row, |ui| {
                    let server = self.sync_server.clone();
                    let k = field(self, ui, "sync-server", &server, "https://notes.example.com", Field::SyncServer);
                    if reset {
                        ui.set_edit_text(k, &server);
                    }
                    self.apply_focus(ui, k, Field::SyncServer);
                    if checking {
                        ui.text("Checking\u{2026}", TextStyle::new(Type::UI.size).color(t.muted).nowrap());
                        pill_button(ui, l, "sync-check-cancel", "Cancel", false, true, Msg::SyncCheckCancel);
                    } else {
                        pill_button(ui, l, "sync-check", "Continue", true, !self.sync_server.trim().is_empty(), Msg::SyncCheck);
                    }
                });
                if let Some(p) = &self.connect.problem {
                    said(ui, "sync-problem", Icon::Warning, t.danger, p.words());
                    ui.leaf(NodeSpec::row().height(12.0).role(Role::None));
                }
            }
            Step::Way => {
                let origin = self.connect.origin().unwrap_or_default().to_string();
                let info = self.connect.info().cloned().unwrap_or_default();
                // The server, as it said it is, and the way back to it.
                ui.with(NodeSpec::row().grow_width().pad_xy(18.0, 8.0).gap(12.0).cross_align(Align::Center).wrap(), |ui| {
                    ui.with(NodeSpec::column().grow_width().min_width(160.0).gap(4.0).selectable(), |ui| {
                        ui.text(&origin, TextStyle::new(Type::STRONG.size).color(t.fg));
                        let (g, ink, words) = if info.fit() == carnet_sync::compat::Fit::UpdateSoon {
                            (Icon::Clock, t.warning, crate::sync::UPDATE_SOON)
                        } else {
                            (Icon::Check, t.muted, "Carnet server, up to date")
                        };
                        ui.with(NodeSpec::row().grow_width().gap(6.0).cross_align(Align::Center), |ui| {
                            crate::icons::icon(ui, g, ink, 14.0, false);
                            ui.text_in(NodeSpec::row().grow_width().min_width(0.0), words, TextStyle::new(Type::SMALL.size).color(ink));
                        });
                    });
                    pill_button(ui, l, "sync-change", "Change", false, true, Msg::SyncChange);
                });
                rule(ui, l);
                ui.text_in(
                    NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 14.0, b: 4.0 }),
                    "Choose sign-in method",
                    TextStyle::new(Type::STRONG.size).color(t.fg),
                );
                // Only the ways this server lists.
                let mut ways = vec![(0, Icon::Device, "Enter the code", "A code from another of your devices, or an invite")];
                if info.takes_passwords() {
                    ways.push((2, Icon::Key, "Email and password", "The ones set on another of your devices"));
                }
                if info.takes_signups() {
                    ways.push((3, Icon::Person, "Make an account", "We'll email you an invite"));
                }
                ui.with(NodeSpec::column().grow_width().padding(Edges { l: 18.0, r: 18.0, t: 6.0, b: 4.0 }).gap(8.0), |ui| {
                    for (to, g, name, what) in ways {
                        choice_row(ui, l, &format!("sync-way-{to}"), g, name, what, Msg::SyncHow { to });
                    }
                });
                ui.with(NodeSpec::row().grow_width().padding(Edges { l: 12.0, r: 18.0, t: 2.0, b: 10.0 }), |ui| {
                    quiet_button(ui, l, "sync-way-1", "Use recovery words", Msg::SyncHow { to: 1 });
                });
            }
            Step::Form => {
                let how = self.sync_how;
                let info = self.connect.info().cloned().unwrap_or_default();
                // An invite just asked for here: said where its code goes.
                let sent = if how == 0 { self.signup.sent_to.clone().zip(self.signup.sent.clone()) } else { None };
                let (title, what) = match how {
                    1 => ("Recovery words", "The twelve words made on one of your devices, for when none is left."),
                    2 if info.password_is_signup_email() => ("Email and password", "The email you signed up with, and the password set on another of your devices."),
                    2 => ("Email and password", "The ones set on another of your devices."),
                    3 => ("Make an account", "We'll email you an invite, with a code to type here."),
                    _ => ("Enter the code", "A code from another of your devices (Settings \u{2192} Sync \u{2192} Add a device), or an invite."),
                };
                let busy = starting || self.signup.sending();
                setting(ui, l, title, if sent.is_some() { None } else { Some(what) }, |ui| {
                    pill_button(ui, l, "sync-back", "Back", false, !busy, Msg::SyncBack);
                });
                if let Some((to, answer)) = &sent {
                    // Asked again too soon: nothing new went, so not "sent".
                    let g = if answer.lately() { Icon::Clock } else { Icon::Check };
                    said(ui, "signup-sent", g, t.fg, &answer.line(to));
                    if let Some(more) = answer.more() {
                        said(ui, "signup-said", Icon::Clock, t.muted, more);
                    }
                }
                ui.with(row, |ui| {
                    match how {
                        3 => {
                            let email = self.signup.email.clone();
                            let k = field(self, ui, "signup-email", &email, "you@example.com", Field::SignupEmail);
                            if reset {
                                ui.set_edit_text(k, &email);
                            }
                            self.apply_focus(ui, k, Field::SignupEmail);
                        }
                        2 => {
                            let email = self.sync_email.clone();
                            let k = field(self, ui, "sync-email", &email, "you@example.com", Field::SyncEmail);
                            if reset {
                                ui.set_edit_text(k, &email);
                            }
                            self.apply_focus(ui, k, Field::SyncEmail);
                            let password = self.sync_password.clone();
                            let k = field(self, ui, "sync-password", &password, "Password", Field::SyncPassword);
                            if reset {
                                ui.set_edit_text(k, &password);
                            }
                        }
                        _ => {
                            // One field for either code: an invite and a
                            // pairing code are told apart by their shape.
                            let hint = if how == 1 { "twelve words, spaces between" } else { "Type or paste the code" };
                            let code = self.sync_code.clone();
                            let k = field(self, ui, "sync-code", &code, hint, Field::SyncCode);
                            if reset {
                                ui.set_edit_text(k, &code);
                            }
                            self.apply_focus(ui, k, Field::SyncCode);
                        }
                    }
                    if how == 3 {
                        if self.signup.sending() {
                            ui.text("Sending\u{2026}", TextStyle::new(Type::UI.size).color(t.muted).nowrap());
                        } else {
                            pill_button(ui, l, "signup-send", "Send me an invite", true, !self.signup.email.trim().is_empty(), Msg::SignupSend);
                        }
                    } else if starting {
                        ui.text("Connecting\u{2026}", TextStyle::new(Type::UI.size).color(t.muted).nowrap());
                        pill_button(ui, l, "sync-cancel", "Cancel", false, true, Msg::SyncCancel);
                    } else {
                        let (button, typed) = match how {
                            1 => ("Recover", !self.sync_code.trim().is_empty()),
                            2 => ("Sign in", !self.sync_email.trim().is_empty() && !self.sync_password.is_empty()),
                            _ => ("Connect", !self.sync_code.trim().is_empty()),
                        };
                        pill_button(ui, l, "sync-start", button, true, typed, Msg::SyncStart);
                    }
                });
                // What went wrong (a wrong code or password), under the
                // field it's of.
                let why = if how == 3 { self.signup.error.clone() } else { self.sync.error.clone() };
                if let Some(why) = why {
                    said(ui, "sync-error", Icon::Warning, t.danger, &why);
                    ui.leaf(NodeSpec::row().height(12.0).role(Role::None));
                }
                // With no ways to choose between, the words are the other
                // way in, from here.
                if !choice && how <= 1 {
                    let (to, word) = if how == 1 { (0, "Use a code instead") } else { (1, "Use recovery words") };
                    ui.with(NodeSpec::row().grow_width().padding(Edges { l: 12.0, r: 18.0, t: 0.0, b: 10.0 }), |ui| {
                        quiet_button(ui, l, &format!("sync-way-{to}"), word, Msg::SyncHow { to });
                    });
                }
                if sent.is_some() && info.takes_signups() {
                    rule(ui, l);
                    setting(ui, l, "No invite yet?", Some("It can take a minute. Look in spam too, or ask again."), |ui| {
                        pill_button(ui, l, "signup-again", "Ask again", false, !starting, Msg::SyncHow { to: 3 });
                    });
                }
            }
        }
        // Anything else said of sync while it's off (it was turned off
        // from elsewhere, say), on the step showing.
        if step != Step::Form
            && let Some(e) = self.sync.error.clone()
        {
            said(ui, "sync-error", Icon::Warning, t.danger, &e);
            ui.leaf(NodeSpec::row().height(12.0).role(Role::None));
        }
    }

    fn sync_settings(&mut self, ui: &mut Ui<'_>, l: &Look) {
        use crate::sync::Status;
        let t = l.t;
        section(ui, l, "Sync");
        let status = self.sync.status.clone();
        let on = self.sync.is_on();
        // A field; a password's has its Show at its right end, an eye.
        let field = |me: &mut Self, ui: &mut Ui<'_>, key: &str, value: &str, hint: &str, f: Field| {
            let secret = matches!(f, Field::SyncPassword | Field::PasswordNew);
            let shown = me.sync_show_password;
            let mut made = None;
            ui.with(
                NodeSpec::row()
                    .grow_width()
                    .min_width(220.0)
                    .height(34.0)
                    .padding(Edges { l: 12.0, r: if secret { 3.0 } else { 8.0 }, t: 0.0, b: 0.0 })
                    .gap(4.0)
                    .radius(radius::CONTROL)
                    .bg(t.sunken)
                    .cross_align(Align::Center),
                |ui| {
                    let k = ui.text_edit(
                        key,
                        value,
                        &EditOptions {
                            style: TextStyle::new(Type::UI.size).color(t.fg),
                            placeholder: Some(hint.into()),
                            // kui's password field: bullets, never copied,
                            // read out as a password, the keyboard kept from
                            // other apps while it has focus.
                            secure: secret && !shown,
                            ..Default::default()
                        },
                        NodeSpec::row().grow_width().min_width(0.0).label(hint),
                    );
                    me.fields.insert(k, f);
                    made = Some(k);
                    if secret {
                        let (g, word) = if shown { (Icon::EyeOff, "Hide password") } else { (Icon::Eye, "Show password") };
                        ui.with_keyed(
                            &format!("{key}-eye"),
                            NodeSpec::row()
                                .size(28.0, 28.0)
                                .center()
                                .radius(radius::CONTROL)
                                .hover_bg(t.sunken.mix(t.fg, 0.06))
                                .pressed_bg(t.sunken.mix(t.fg, 0.1))
                                .on_click(Msg::SyncShowPassword)
                                .role(Role::Checkbox)
                                .checked(shown)
                                .label(word)
                                .tooltip(word),
                            |ui| crate::icons::icon(ui, g, t.muted, 18.0, false),
                        );
                    }
                },
            );
            made.expect("the field is made")
        };
        card(ui, l, "card-sync", |ui| {
            if status == Status::Unlocking {
                setting(
                    ui,
                    l,
                    "Waiting for the keychain\u{2026}",
                    Some("This device's sync keys are in the system's keychain, which may be asking for your password first."),
                    |_| {},
                );
            } else if !on {
                let starting = status == Status::Starting;
                self.sync_way_in(ui, l, starting, &field);
            } else {
                let (word, ink) = match &status {
                    Status::Online => ("Online".to_string(), t.fg),
                    Status::Connecting | Status::Starting => ("Connecting\u{2026}".to_string(), t.muted),
                    Status::Unlocking => ("Waiting for the keychain\u{2026}".to_string(), t.muted),
                    Status::Offline(_) => ("Offline".to_string(), t.muted),
                    Status::Off => ("Off".to_string(), t.muted),
                };
                let whom = format!(
                    "{} \u{b7} account {}",
                    self.sync.origin().unwrap_or_default(),
                    self.sync.account().unwrap_or_default()
                );
                let usage = self.sync.usage.clone();
                let read_only = self.sync.read_only;
                let update_soon = self.sync.update_soon;
                let waiting = (self.sync.waiting(), self.sync.pictures_waiting());
                let (wait_said, again) = self.wait_line.said(ui.now(), waiting != (0, 0));
                if let Some(at) = again {
                    ui.request_frame_at(at);
                }
                let in_file = self.sync.keys_in_file();
                // A line said of the account: an icon, then words.
                let said = |ui: &mut Ui<'_>, g: Icon, ink: Color, words: &str| {
                    ui.with(NodeSpec::row().grow_width().gap(6.0).cross_align(Align::Center), |ui| {
                        crate::icons::icon(ui, g, ink, 14.0, false);
                        ui.text_in(NodeSpec::row().grow_width().min_width(0.0), words, TextStyle::new(Type::SMALL.size).color(ink));
                    });
                };
                ui.with(NodeSpec::column().grow_width().pad_xy(18.0, 14.0).gap(4.0).selectable(), |ui| {
                    ui.text(&word, TextStyle::new(Type::STRONG.size).color(ink));
                    ui.text(&whom, TextStyle::new(Type::SMALL.size).color(t.muted));
                    // Why offline, in plain words; what the network said,
                    // if anything, only on hover.
                    if let Status::Offline(why) = &status {
                        let mut spec = NodeSpec::row().grow_width().gap(6.0).cross_align(Align::Center);
                        if let Some(detail) = &self.sync.offline_detail {
                            spec = spec.tooltip(detail);
                        }
                        ui.with_keyed("sync-offline", spec, |ui| {
                            crate::icons::icon(ui, Icon::Warning, t.muted, 14.0, false);
                            ui.text_in(NodeSpec::row().grow_width().min_width(0.0), why, TextStyle::new(Type::SMALL.size).color(t.muted));
                        });
                    }
                    // Changes the server isn't known to keep yet: not "up
                    // to date" while any wait — once they have waited a
                    // moment, and then "All sent" a moment more.
                    match wait_said {
                        Some(Waited::Waiting) => said(ui, Icon::Clock, t.muted, &waiting_words(waiting.0, waiting.1)),
                        Some(Waited::Sent) => said(ui, Icon::Check, t.muted, ALL_SENT),
                        None => {}
                    }
                    // What the server keeps for the account, and, on a
                    // hosted plan, how much it may.
                    if let Some(u) = &usage {
                        let kept = size_words(u.notes + u.pictures);
                        let mut line = match u.limit {
                            Some(limit) => format!("{kept} of {} used", size_words(limit)),
                            None => format!("{kept} kept on the server"),
                        };
                        if !u.plan.is_empty() {
                            line.push_str(&format!(" \u{b7} {}", u.plan));
                            if let Some(until) = u.until {
                                line.push_str(&format!(", until {}", super::chrome::date_words(SystemTime::UNIX_EPOCH + Duration::from_secs(until))));
                            }
                        }
                        said(ui, Icon::Cloud, t.muted, &line);
                        if u.writable
                            && !read_only
                            && let Some(limit) = u.limit
                            && let Some(over) = over_limit(u.notes + u.pictures, limit)
                        {
                            said(ui, Icon::Warning, t.warning, &over);
                        }
                    }
                    // Read only: said here, and only here (not again as
                    // an error below the card).
                    if read_only || usage.as_ref().is_some_and(|u| !u.writable) {
                        said(ui, Icon::Warning, t.danger, crate::sync::READ_ONLY);
                    }
                    if update_soon {
                        ui.text(crate::sync::UPDATE_SOON, TextStyle::new(Type::SMALL.size).color(t.danger));
                    }
                    // Linux with no keychain running: where the keys are
                    // instead.
                    if in_file {
                        said(ui, Icon::Key, t.warning, crate::sync::KEYS_IN_FILE);
                    }
                });
                rule(ui, l);
                // The account's devices: each by the name it gave, this one
                // marked; any other can be removed (asked twice).
                match self.sync.devices.clone() {
                    None => {
                        ui.text_in(
                            NodeSpec::row().pad_xy(18.0, 14.0),
                            "Listing this account's devices\u{2026}",
                            TextStyle::new(Type::SMALL.size).color(t.muted),
                        );
                    }
                    Some(list) => {
                        ui.text_in(
                            NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 12.0, b: 0.0 }),
                            "Devices",
                            TextStyle::new(Type::SMALL.size).color(t.muted),
                        );
                        let now = SystemTime::now();
                        // Each list in this card keyed by its own name and
                        // the row's place: an index key alone is the one the
                        // `i`th unkeyed child has, and two lists' first rows
                        // would be one node to the access tree.
                        for (i, d) in list.iter().enumerate() {
                            let name = d.name.clone().unwrap_or_else(|| "A device not yet named".into());
                            let what = if d.you {
                                "This device".to_string()
                            } else if d.online && status == Status::Online {
                                // Logged in now: not when its session began.
                                // (Offline here, the list is an old one: it
                                // can't say who's online now.)
                                "Online now".to_string()
                            } else if d.seen == 0 {
                                "Not seen since it came in".to_string()
                            } else {
                                format!("Last seen {}", super::chrome::ago(now, SystemTime::UNIX_EPOCH + Duration::from_secs(d.seen)))
                            };
                            // Its id, short: two devices of one name told apart.
                            let short: String = d.id.iter().take(3).map(|b| format!("{b:02x}")).collect();
                            let what = format!("{what} \u{b7} {short}");
                            let armed = self.sync_remove_armed.as_deref() == Some(d.id.as_slice());
                            let what = if armed {
                                format!("{what}. Removed, it can't sync, nor read anything written from now on; its own copy of the notes stays on it.")
                            } else {
                                what
                            };
                            // This device's name, being changed: in a field.
                            if d.you
                                && let Some(typed) = self.sync_rename.clone()
                            {
                                ui.with_key(ui.child_key("sync-device").index(i as u64), NodeSpec::column().grow_width(), |ui| {
                                    ui.with(NodeSpec::row().grow_width().pad_xy(18.0, 12.0).gap(10.0).cross_align(Align::Center).wrap(), |ui| {
                                        let k = field(self, ui, "sync-rename", &typed, "What this device is called", Field::DeviceName);
                                        self.apply_focus(ui, k, Field::DeviceName);
                                        pill_button(ui, l, "sync-rename-cancel", "Cancel", false, true, Msg::SyncRenameCancel);
                                        pill_button(ui, l, "sync-rename-save", "Save", true, status == Status::Online, Msg::SyncRenameSave);
                                    });
                                });
                                continue;
                            }
                            ui.with_key(ui.child_key("sync-device").index(i as u64), NodeSpec::column().grow_width(), |ui| {
                                setting(ui, l, &name, Some(&what), |ui| {
                                    if d.you {
                                        pill_button(ui, l, "sync-rename", "Rename", false, status == Status::Online, Msg::SyncRename);
                                    } else {
                                        let word = if armed { "Remove for real" } else { "Remove" };
                                        pill_button(ui, l, &format!("sync-remove-{i}"), word, armed, status == Status::Online, Msg::SyncRemove { at: i as u64 });
                                    }
                                });
                            });
                        }
                    }
                }
                rule(ui, l);
                let code = self.sync.code.clone();
                setting(ui, l, "Add a device", Some("Show a code here, then scan it with the new device's camera, or type it there. It works once."), |ui| {
                    if code.is_some() {
                        pill_button(ui, l, "sync-pair-cancel", "Cancel", false, true, Msg::SyncPairCancel);
                    } else {
                        pill_button(ui, l, "sync-pair", "Show a code", false, status == Status::Online, Msg::SyncPair);
                    }
                });
                // Said while the device is in the list: one that left
                // since isn't "in the list above".
                let joined_listed = self.sync.joined.as_deref().is_some_and(|j| {
                    self.sync.devices.as_ref().is_none_or(|list| list.iter().any(|d| d.id.iter().map(|b| format!("{b:02x}")).collect::<String>() == j))
                });
                if code.is_none() && joined_listed {
                    ui.text_in(
                        NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 0.0, b: 14.0 }).selectable().live(kui_native::Live::Polite),
                        "A new device is in: it's in the list above.",
                        TextStyle::new(Type::UI.size).color(t.fg),
                    );
                }
                // The code as a QR code too: a link to the web viewer on
                // this server with the code in it, for a phone's camera.
                let link = code.as_deref().and_then(carnet_sync::pair::PairCode::parse).zip(self.sync.origin()).map(|(c, o)| c.link(o));
                if self.pair_qr.as_ref().map(|(l, _)| l) != link.as_ref() {
                    let core = ui.core();
                    if let Some((_, id)) = self.pair_qr.take() {
                        core.remove_image(id);
                    }
                    self.pair_qr = link.and_then(|link| {
                        let (w, rgba) = qr_rgba(&link)?;
                        Some((link, core.resources.add_image(w, w, rgba)))
                    });
                }
                if let Some(code) = &code {
                    ui.with(NodeSpec::row().grow_width().gap(20.0).cross_align(Align::Center).padding(Edges { l: 18.0, r: 18.0, t: 0.0, b: 14.0 }), |ui| {
                        if let Some((_, qr)) = &self.pair_qr {
                            ui.image(*qr, NodeSpec::row().size(150.0, 150.0).radius(6.0).label("The code as a QR code"));
                        }
                        ui.text_in(
                            NodeSpec::row().selectable().label("Pairing code"),
                            code,
                            l.page_type(Type::TITLE).family(l.serif).color(t.fg),
                        );
                    });
                }
                rule(ui, l);
                let words = self.sync.words.clone();
                let armed = self.sync_words_armed;
                let making = self.sync.making_recovery();
                // One there already (or not known not to be): what the
                // button does is replace it, and the confirm asks first.
                let has = self.sync.has_recovery();
                let what = if armed {
                    "Replace the recovery code? The words you have now will stop working."
                } else if has {
                    "This account has one: twelve words that let a new device in when none of yours is left."
                } else {
                    "Twelve words that let a new device in when none of yours is left."
                };
                setting(ui, l, "Recovery code", Some(what), |ui| {
                    if words.is_some() {
                        pill_button(ui, l, "sync-words-done", "Done", true, true, Msg::SyncRecoveryDone);
                    } else if armed {
                        pill_button(ui, l, "sync-words-cancel", "Cancel", false, true, Msg::SyncRecoveryCancel);
                        pill_button(ui, l, "sync-words-yes", "Yes, replace", true, status == Status::Online, Msg::SyncRecovery);
                    } else if making {
                        pill_button(ui, l, "sync-words", "Making one\u{2026}", false, false, Msg::SyncRecovery);
                    } else {
                        pill_button(ui, l, "sync-words", if has { "Replace" } else { "Make one" }, false, status == Status::Online, Msg::SyncRecovery);
                    }
                });
                if let Some(words) = &words {
                    ui.with(NodeSpec::column().grow_width().padding(Edges { l: 18.0, r: 18.0, t: 0.0, b: 14.0 }).gap(6.0), |ui| {
                        // Numbered, four to a row: written down by hand, a
                        // place is easy to lose in one long line.
                        let words: Vec<&str> = words.split_whitespace().collect();
                        ui.with(NodeSpec::column().grow_width().gap(6.0).selectable().label("Recovery words"), |ui| {
                            for (r, row) in words.chunks(4).enumerate() {
                                ui.with(NodeSpec::row().grow_width(), |ui| {
                                    for (c, w) in row.iter().enumerate() {
                                        ui.with(NodeSpec::row().width(kui_native::Sizing::Percent(0.25)).gap(8.0).cross_align(Align::Baseline), |ui| {
                                            ui.with(NodeSpec::row().width(kui_native::Sizing::Fixed(20.0)).main_align(Align::End), |ui| {
                                                ui.text(&format!("{}", r * 4 + c + 1), TextStyle::new(Type::SMALL.size).color(t.muted).nowrap());
                                            });
                                            ui.text(w, l.page_type(Type::BODY).family(l.serif).color(t.fg).nowrap());
                                        });
                                    }
                                });
                            }
                        });
                        ui.text(
                            "Write them down and keep them somewhere safe: they are shown this once, and kept nowhere on this device. A new code replaces this one.",
                            TextStyle::new(Type::SMALL.size).color(t.muted),
                        );
                    });
                }
                // An email and password to come in by (a server that takes
                // them: a hosted one), beside codes and the words.
                if let Some((true, id)) = self.sync.password.clone() {
                    rule(ui, l);
                    // A hosted service holds passwords to the email signed
                    // up with: shown, never asked.
                    let only = self.sync.password_email.clone();
                    let what = match (&id, &only) {
                        (Some(id), _) => format!("A new device, or a browser, can come in with {id} and the password."),
                        (None, Some(only)) => format!("Set one, and a new device, or a browser, can come in with {only} and it, instead of a code."),
                        (None, None) => "Set one, and a new device, or a browser, can come in with your email and it, instead of a code.".to_string(),
                    };
                    let online = status == Status::Online;
                    let editing = self.pw_edit.is_some();
                    let armed = self.pw_remove_armed;
                    setting(ui, l, "Email and password", Some(&what), |ui| {
                        if editing {
                            return;
                        }
                        if armed {
                            pill_button(ui, l, "pw-remove-cancel", "Cancel", false, true, Msg::SyncPasswordCancel);
                            pill_button(ui, l, "pw-remove-yes", "Yes, take it away", true, online, Msg::SyncPasswordRemove);
                        } else if id.is_some() {
                            pill_button(ui, l, "pw-remove", "Take away", false, online, Msg::SyncPasswordRemove);
                            pill_button(ui, l, "pw-change", "Change", false, online, Msg::SyncPassword);
                        } else {
                            pill_button(ui, l, "pw-set", "Set one", false, online, Msg::SyncPassword);
                        }
                    });
                    if let Some((email, password)) = self.pw_edit.clone() {
                        ui.with(NodeSpec::row().grow_width().padding(Edges { l: 18.0, r: 18.0, t: 0.0, b: 10.0 }).gap(10.0).cross_gap(10.0).cross_align(Align::Center).wrap(), |ui| {
                            if let Some(only) = &only {
                                ui.text(only, TextStyle::new(Type::UI.size).color(t.fg).nowrap());
                            } else {
                                let k = field(self, ui, "pw-email", &email, "you@example.com", Field::PasswordEmail);
                                if self.focus_req == Some(Field::PasswordEmail) {
                                    ui.focus(k);
                                    self.focus_req = None;
                                }
                            }
                            let hint = format!("A password, {} letters or more", carnet_sync::password::MIN_LEN);
                            let k = field(self, ui, "pw-new", &password, &hint, Field::PasswordNew);
                            if self.focus_req == Some(Field::PasswordNew) {
                                ui.focus(k);
                                self.focus_req = None;
                            }
                        });
                        if only.is_some() {
                            ui.text_in(
                                NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 0.0, b: 6.0 }),
                                "On this service, the password signs in with the email you signed up with.",
                                TextStyle::new(Type::SMALL.size).color(t.muted),
                            );
                        }
                        ui.text_in(
                            NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 0.0, b: 10.0 }),
                            "The password never leaves this device: the server can't read it, nor reset it. Keep the recovery words too.",
                            TextStyle::new(Type::SMALL.size).color(t.muted),
                        );
                        // The row's own buttons, under its fields, whatever the width.
                        let ready = only.as_deref().unwrap_or(&email).contains('@') && password.chars().count() >= 8;
                        ui.with(NodeSpec::row().grow_width().padding(Edges { l: 18.0, r: 18.0, t: 0.0, b: 14.0 }).gap(10.0).main_align(Align::End), |ui| {
                            pill_button(ui, l, "pw-cancel", "Cancel", false, true, Msg::SyncPasswordCancel);
                            pill_button(ui, l, "pw-save", "Save", true, ready && online, Msg::SyncPasswordSave);
                        });
                    }
                }
                // Passkeys, made in a browser on this server's web page (this
                // app can't make one): each by its name, removed when asked
                // twice, Cancel or Yes.
                let passkeys = self.sync.passkeys.clone();
                if !passkeys.is_empty() && self.sync.server_has(feature::PASSKEYS) {
                    rule(ui, l);
                    ui.text_in(
                        NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 12.0, b: 0.0 }),
                        "Passkeys",
                        TextStyle::new(Type::SMALL.size).color(t.muted),
                    );
                    let online = status == Status::Online;
                    let now = SystemTime::now();
                    for (i, p) in passkeys.iter().enumerate() {
                        let name = p.name.clone().unwrap_or_else(|| "A passkey".into());
                        let armed = self.sync_passkey_armed == Some(p.id);
                        let what = if armed {
                            "Removed, it can't sign in to this account any more. It stays in the password manager that keeps it until it's deleted there.".to_string()
                        } else if p.made == 0 {
                            "Signs a browser in to this account".to_string()
                        } else {
                            let added = super::chrome::ago(now, SystemTime::UNIX_EPOCH + Duration::from_secs(p.made));
                            format!("Added {added}. Signs a browser in to this account.")
                        };
                        ui.with_key(ui.child_key("sync-passkey").index(i as u64), NodeSpec::column().grow_width(), |ui| {
                            setting(ui, l, &name, Some(&what), |ui| {
                                if armed {
                                    pill_button(ui, l, &format!("passkey-cancel-{i}"), "Cancel", false, true, Msg::SyncPasskeyCancel);
                                    pill_button(ui, l, &format!("passkey-yes-{i}"), "Yes, remove", true, online, Msg::SyncPasskeyRemove { at: i as u64 });
                                } else {
                                    pill_button(ui, l, &format!("passkey-remove-{i}"), "Remove", false, online, Msg::SyncPasskeyRemove { at: i as u64 });
                                }
                            });
                        });
                    }
                }
                // The server's owner (its first account) lets other people
                // on: invites, and the accounts there. Nobody else sees this.
                if let Some(inv) = self.sync.invites.clone().filter(|i| i.owner) {
                    rule(ui, l);
                    let fresh = self.sync.invite.clone();
                    setting(
                        ui,
                        l,
                        "Invite someone",
                        Some("An invite makes a new account on this server, for someone else's own notes. It works once, within a week."),
                        |ui| {
                            if fresh.is_some() {
                                pill_button(ui, l, "sync-invite-done", "Done", true, true, Msg::SyncInviteDone);
                            } else {
                                pill_button(ui, l, "sync-invite", "Make an invite", false, status == Status::Online, Msg::SyncInvite);
                            }
                        },
                    );
                    if let Some(code) = &fresh {
                        ui.with(NodeSpec::column().grow_width().padding(Edges { l: 18.0, r: 18.0, t: 0.0, b: 14.0 }).gap(6.0), |ui| {
                            ui.text_in(NodeSpec::row().selectable().label("Invite code"), code, l.page_type(Type::TITLE).family(l.serif).color(t.fg));
                            let origin = self.sync.origin().unwrap_or_default();
                            ui.text(
                                &format!("Give them this code and the server's address, {origin}. In Carnet's Settings \u{2192} Sync they type both, and Connect."),
                                TextStyle::new(Type::SMALL.size).color(t.muted),
                            );
                        });
                    }
                    let now = SystemTime::now();
                    let secs = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
                    // Each waiting: its code, to select or copy, kept in the
                    // vault by whichever of the account's devices made it
                    // (one made before they were kept there is known by its
                    // hash only).
                    for (i, w) in inv.waiting.iter().enumerate() {
                        let code = self.sync.invite_code(&self.store, &w.hash);
                        let title = code.clone().unwrap_or_else(|| "An invite".into());
                        let what = format!("Not used yet \u{b7} {}", left(secs, w.until));
                        ui.with_key(ui.child_key("sync-waiting").index(i as u64), NodeSpec::column().grow_width(), |ui| {
                            setting_in(ui, l, &title, Some(&what), code.is_some(), |ui| {
                                if code.is_some() {
                                    pill_button(ui, l, &format!("sync-invite-copy-{i}"), "Copy", false, true, Msg::SyncInviteCopy { at: i as u64 });
                                }
                                pill_button(ui, l, &format!("sync-invite-drop-{i}"), "Take back", false, status == Status::Online, Msg::SyncInviteDrop { at: i as u64 });
                            });
                        });
                    }
                    let others: Vec<_> = inv.accounts.iter().enumerate().filter(|(_, a)| !a.you).collect();
                    if !others.is_empty() {
                        rule(ui, l);
                        ui.text_in(
                            NodeSpec::row().padding(Edges { l: 18.0, r: 18.0, t: 12.0, b: 0.0 }),
                            "Other accounts on this server",
                            TextStyle::new(Type::SMALL.size).color(t.muted),
                        );
                        for (i, a) in others {
                            let devices = if a.devices == 1 { "1 device".to_string() } else { format!("{} devices", a.devices) };
                            let seen = if a.seen == 0 {
                                "not seen yet".to_string()
                            } else {
                                format!("last seen {}", super::chrome::ago(now, SystemTime::UNIX_EPOCH + Duration::from_secs(a.seen)))
                            };
                            let short: String = a.id.iter().take(3).map(|b| format!("{b:02x}")).collect();
                            let armed = self.sync_account_armed.as_deref() == Some(a.id.as_slice());
                            let what = format!("{devices}, {seen} \u{b7} {short}");
                            let what = if armed {
                                format!("{what}. Removed, everything it keeps on this server goes: its notes there, its pictures, its devices' way in. What's on their devices stays.")
                            } else {
                                what
                            };
                            ui.with_key(ui.child_key("sync-account").index(i as u64), NodeSpec::column().grow_width(), |ui| {
                                setting(ui, l, "An account", Some(&what), |ui| {
                                    let word = if armed { "Remove for real" } else { "Remove" };
                                    pill_button(ui, l, &format!("sync-account-remove-{i}"), word, armed, status == Status::Online, Msg::SyncAccountRemove { at: i as u64 });
                                });
                            });
                        }
                    }
                }
                rule(ui, l);
                let open = self.sync_inspector;
                setting(ui, l, "Notes on the server", Some("Whether each note is up to date, and how to fix one that looks wrong."), |ui| {
                    if open {
                        pill_button(ui, l, "sync-inspect-refresh", "Check again", false, status == Status::Online, Msg::SyncInspectorRefresh);
                    }
                    pill_button(ui, l, "sync-inspect", if open { "Hide" } else { "Show" }, false, true, Msg::SyncInspector);
                });
                if open {
                    self.sync_inspector_view(ui, l);
                }
                rule(ui, l);
                let armed = self.sync_off_armed;
                let what = if armed {
                    "Its keys are forgotten: to come back it needs a code from another device, or the recovery words. With neither, the synced copy can't be reached again."
                } else {
                    "Turned off, this device stops syncing; its notes stay as they are."
                };
                setting(ui, l, "On this device", Some(what), |ui| {
                    pill_button(ui, l, "sync-off", if armed { "Turn off for real" } else { "Turn off" }, armed, true, Msg::SyncOff);
                });
                // The whole account, from any of its devices; not the
                // server's owner's, which lets everyone else on.
                if !self.sync.invites.as_ref().is_some_and(|i| i.owner) && self.sync.server_has(feature::ACCOUNT_DELETE) {
                    rule(ui, l);
                    let armed = self.delete_account_armed;
                    let what = if armed {
                        "Every note and picture the server keeps goes, for good, and every device leaves the account. The notes stay here as files, and on each device as they are."
                    } else {
                        "Everything the server keeps of it, on every device."
                    };
                    let online = status == Status::Online;
                    setting(ui, l, "Delete the account", Some(what), |ui| {
                        if armed {
                            pill_button(ui, l, "account-delete-cancel", "Cancel", false, true, Msg::SyncDeleteAccountCancel);
                            pill_button(ui, l, "account-delete-yes", "Yes, delete the account", true, online, Msg::SyncDeleteAccount);
                        } else {
                            pill_button(ui, l, "account-delete", "Delete", false, online, Msg::SyncDeleteAccount);
                        }
                    });
                }
            }
        });
        // (While off, said in the card, on the step it's of.)
        if (on || status == Status::Unlocking)
            && let Some(e) = &self.sync.error
        {
            ui.text_in(NodeSpec::row().pad_xy(4.0, 0.0).selectable().live(kui_native::Live::Polite), e, TextStyle::new(Type::SMALL.size).color(t.danger));
        }
        ui.text_in(
            NodeSpec::row().pad_xy(4.0, 0.0).selectable(),
            "Sync is end-to-end encrypted: the server keeps your notes sealed, and never sees a word of them. They stay plain files in your notes folder either way.",
            TextStyle::new(Type::SMALL.size).color(t.muted),
        );
    }

    /// The faces to choose from, searchable, each name set in its own
    /// face: the default first, then every installed family that matches.
    fn font_list(&mut self, ui: &mut Ui<'_>, l: &Look, which: u64, current: &str) {
        let t = l.t;
        let families = self
            .families
            .get_or_insert_with(|| {
                let mut all = ui.core().system_font_families();
                // The suggested ones first, in their order, when installed.
                let mut first: Vec<String> = look::SUGGESTED_FONTS
                    .iter()
                    .filter(|s| all.iter().any(|a| a == *s))
                    .map(|s| s.to_string())
                    .collect();
                all.retain(|a| !first.contains(a) && !a.starts_with('.') && !a.starts_with('@'));
                first.extend(all);
                first
            })
            .clone();
        let q = self.font_query.trim().to_lowercase();
        // The built-in face once, as the default: installed under its own
        // name it is the same face.
        let own = built_in(which);
        self.font_rows = families
            .into_iter()
            .filter(|f| Some(f.as_str()) != own && (q.is_empty() || f.to_lowercase().contains(&q)))
            .collect();
        let default_name = default_font_name(which);
        let default_found = own.is_some_and(|f| f.to_lowercase().contains(&q));
        let style = TextStyle::new(Type::UI.size).color(t.fg);
        ui.with(
            NodeSpec::column()
                .grow_width()
                .padding(Edges { l: 14.0, r: 14.0, t: 0.0, b: 12.0 })
                .gap(6.0),
            |ui| {
                // The search, as the library's is.
                ui.with(
                    NodeSpec::row()
                        .grow_width()
                        .height(32.0)
                        .padding(Edges { l: 12.0, r: 6.0, t: 0.0, b: 0.0 })
                        .radius(radius::CONTROL)
                        .bg(t.sunken)
                        .cross_align(Align::Center),
                    |ui| {
                        // The field's own placeholder, as the library's:
                        // where the caret and the typed words start.
                        let k = ui.text_edit(
                            "font-search",
                            "",
                            &EditOptions { style, placeholder: Some("Search fonts".into()), ..Default::default() },
                            NodeSpec::row().grow_width().label("Search fonts"),
                        );
                        if std::mem::take(&mut self.font_search_reset) {
                            ui.set_edit_text(k, "");
                        }
                        self.fields.insert(k, Field::Font);
                    },
                );
                let row = |ui: &mut Ui<'_>, key: u64, name: &str, face: FontFamily, on: bool, msg: Msg| {
                    ui.with_indexed(
                        key,
                        NodeSpec::row()
                            .grow_width()
                            .height(FONT_ROW_H - 2.0)
                            .pad_xy(12.0, 0.0)
                            .radius(radius::CONTROL)
                            .cross_align(Align::Center)
                            .bg(if on { t.accent_soft } else { Color::TRANSPARENT })
                            .hover_bg(if on { t.accent_soft } else { t.hover })
                            .on_click(msg)
                            .selected(on)
                            .label(name),
                        |ui| {
                            ui.text_in(
                                NodeSpec::row().grow_width().min_width(0.0),
                                name,
                                TextStyle::new(Type::STRONG.size).family(face).color(t.fg).ellipsis(),
                            );
                        },
                    );
                };
                // The default row in the default face: the letter's is
                // the headings' as they are now.
                let fallback = match which {
                    0 => Self::face_of(&mut self.font_faces, ui, look::BODY_FACE),
                    1 => Self::face_of(&mut self.font_faces, ui, look::HEADING_FACE),
                    _ => l.serif,
                };
                row(ui, u64::MAX - 1, default_name, fallback, is_built_in(which, current), Msg::SetFont { which, index: ALL });
                let (rows, faces) = (&self.font_rows, &mut self.font_faces);
                if rows.is_empty() && default_found {
                    return;
                }
                if rows.is_empty() {
                    ui.text_in(
                        NodeSpec::row().pad_xy(12.0, 6.0),
                        "No installed font by that name",
                        TextStyle::new(Type::SMALL.size).color(t.muted),
                    );
                    return;
                }
                widgets::uniform_list_with(
                    ui,
                    ["fonts-body", "fonts-head", "fonts-initial"][(which as usize).min(2)],
                    NodeSpec::column()
                        .grow_width()
                        .height((rows.len() as f32 * FONT_ROW_H).min(FONT_ROW_H * 7.0))
                        .scrollbar_width(4.0)
                        .role(Role::List)
                        .label("Fonts"),
                    rows.len(),
                    FONT_ROW_H,
                    |_| NodeSpec::column().pad_xy(0.0, 1.0),
                    |ui, i| {
                        let name = &rows[i];
                        let face = Self::face_of(faces, ui, name);
                        row(ui, i as u64, name, face, *name == current, Msg::SetFont { which, index: i as u64 });
                    },
                );
            },
        );
    }
}

/// The face a font list's default row stands for, by its family name:
/// the text's and the headings' are shipped; the letter's is the
/// headings', whatever they are.
fn built_in(which: u64) -> Option<&'static str> {
    match which {
        0 => Some(look::BODY_FACE),
        1 => Some(look::HEADING_FACE),
        _ => None,
    }
}

/// A font list's default row, and its pill while it is chosen.
fn default_font_name(which: u64) -> &'static str {
    match which {
        0 => "Inter (built in)",
        1 => "Source Serif 4 (built in)",
        _ => "Same as headings",
    }
}

/// `name`, as the settings keep it, is the list's default face: none
/// chosen, or the built-in one by its name.
fn is_built_in(which: u64, name: &str) -> bool {
    name.is_empty() || built_in(which) == Some(name)
}

fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| p.to_string_lossy().trim_end_matches(['/', '\\']).to_lowercase();
    norm(a) == norm(b)
}

/// A small heading over a card.
fn section(ui: &mut Ui<'_>, l: &Look, name: &str) {
    ui.text_in(
        NodeSpec::row().padding(Edges { l: 4.0, r: 0.0, t: 18.0, b: 2.0 }),
        name,
        TextStyle::new(Type::SMALL.size).color(l.t.muted).nowrap(),
    );
}

/// Settings grouped on one card, rows split by hairlines.
fn card(ui: &mut Ui<'_>, l: &Look, key: &str, f: impl FnOnce(&mut Ui<'_>)) {
    ui.with_keyed(
        key,
        NodeSpec::column()
            .grow_width()
            .radius(radius::CARD)
            .bg(l.card)
            .border(1.0, l.t.border)
            .shadow_color(l.shadow)
            .shadow_blur(14.0)
            .shadow_y(2.0)
            .clip(),
        f,
    );
}

fn rule(ui: &mut Ui<'_>, l: &Look) {
    ui.leaf(NodeSpec::row().grow_width().height(1.0).bg(l.t.border).role(Role::None));
}

/// One setting: its name (and a word on it) on the left, its control on
/// the right.
fn setting(ui: &mut Ui<'_>, l: &Look, name: &str, note: Option<&str>, control: impl FnOnce(&mut Ui<'_>)) {
    setting_in(ui, l, name, note, false, control);
}

/// [`setting`], its name and word on it selectable, to copy (a code).
fn setting_in(ui: &mut Ui<'_>, l: &Look, name: &str, note: Option<&str>, selectable: bool, control: impl FnOnce(&mut Ui<'_>)) {
    let t = l.t;
    ui.with(
        NodeSpec::row()
            .grow_width()
            .min_height(54.0)
            .pad_xy(18.0, 8.0)
            .gap(12.0)
            .cross_align(Align::Center)
            .wrap(),
        |ui| {
            let words = NodeSpec::column().grow_width().min_width(120.0).gap(2.0);
            ui.with(if selectable { words.selectable() } else { words }, |ui| {
                ui.text(name, TextStyle::new(Type::STRONG.size).color(t.fg));
                if let Some(n) = note {
                    ui.text(n, TextStyle::new(Type::SMALL.size).color(t.muted));
                }
            });
            control(ui);
        },
    );
}

/// Settings that belong to the one above them — shown only while it is
/// on, or choosing its parts one by one: set in under its name, a guide
/// down their left, no hairlines between them or above.
fn subgroup(ui: &mut Ui<'_>, l: &Look, key: &str, name: &str, f: impl FnOnce(&mut Ui<'_>)) {
    ui.with_keyed(
        key,
        NodeSpec::row()
            .grow_width()
            .padding(Edges { l: 18.0, r: 0.0, t: 0.0, b: design::space::M })
            .role(Role::Group)
            .label(name),
        |ui| {
            ui.leaf(NodeSpec::column().width(2.0).grow_height().radius(radius::ROUND).bg(l.t.border).role(Role::None));
            ui.with(NodeSpec::column().grow_width().min_width(0.0), f);
        },
    );
}

/// One setting in a subgroup: a name and its control, a row lower than
/// a setting's.
fn sub_setting(ui: &mut Ui<'_>, l: &Look, name: &str, control: impl FnOnce(&mut Ui<'_>)) {
    ui.with(
        NodeSpec::row()
            .grow_width()
            .min_height(44.0)
            .padding(Edges { l: design::space::L, r: 18.0, t: design::space::XS, b: design::space::XS })
            .gap(design::space::M)
            .cross_align(Align::Center)
            .wrap(),
        |ui| {
            ui.text_in(NodeSpec::row().grow_width().min_width(80.0), name, TextStyle::new(Type::UI.size).color(l.t.fg));
            control(ui);
        },
    );
}

/// A palette as a small page: its library column, its page, a card on it
/// and an accent dot, in the mode showing; its name under it.
fn swatch(ui: &mut Ui<'_>, l: &Look, i: usize, p: design::Palette, on: bool) {
    let t = l.t;
    let c = design::Colors::of(p, l.c.dark);
    ui.with_indexed(
        i as u64,
        NodeSpec::column()
            .gap(design::space::XS)
            .cross_align(Align::Center)
            .on_click(Msg::SetPalette { to: i as u64 })
            .role(Role::Checkbox)
            .checked(on)
            .label(p.name()),
        |ui| {
            // The border is a frame round the page, not under it: children
            // paint over their parent's border, so the page sits inside.
            const RING: f32 = 2.0;
            ui.with(
                NodeSpec::row()
                    .size(58.0, 40.0)
                    .radius(design::radius::CONTROL)
                    .border(RING, if on { l.c.accent_ink } else { l.c.border })
                    .pad(RING),
                |ui| {
                    ui.with(
                        NodeSpec::row()
                            .grow_width()
                            .grow_height()
                            .radius(design::radius::CONTROL - RING)
                            .bg(c.page)
                            .clip(),
                        |ui| {
                            ui.leaf(NodeSpec::row().width(18.0 - RING).grow_height().bg(c.side));
                            ui.with(
                                NodeSpec::column().grow_width().grow_height().pad(design::space::XS).gap(3.0),
                                |ui| {
                                    ui.leaf(NodeSpec::row().grow_width().height(3.0).radius(1.5).bg(c.ink.with_alpha(0.75)));
                                    ui.leaf(NodeSpec::row().width(18.0).height(3.0).radius(1.5).bg(c.muted.with_alpha(0.6)));
                                    ui.leaf(NodeSpec::row().size(8.0, 8.0).radius(4.0).bg(c.accent));
                                },
                            );
                        },
                    );
                },
            );
            ui.text(
                p.name(),
                TextStyle::new(Type::SMALL.size).color(if on { t.fg } else { t.muted }).nowrap(),
            );
        },
    );
}

/// A way to draw dividers, drawn that way; the chosen one ringed.
fn divider_choice(ui: &mut Ui<'_>, l: &Look, i: usize, d: crate::settings::Divider, on: bool) {
    let t = l.t;
    ui.with_indexed(
        i as u64,
        NodeSpec::row()
            .size(72.0, 40.0)
            .center()
            .radius(design::radius::CONTROL)
            .bg(t.bg)
            .border(2.0, if on { l.c.accent_ink } else { l.c.border })
            .hover_bg(t.hover)
            .on_click(Msg::SetDivider { to: i as u64 })
            .role(Role::Checkbox)
            .checked(on)
            .label(d.name())
            .tooltip(d.name()),
        // As on the page, or smaller where it would not fit.
        |ui| {
            let k = (56.0 / crate::icons::ornament_width(d)).min(1.0);
            crate::icons::ornament(ui, d, if on { l.c.accent_ink } else { t.muted }, k)
        },
    );
}

/// A row of choices, one on.
fn segmented(ui: &mut Ui<'_>, l: &Look, key: &str, options: &[&str], current: usize, msg: impl Fn(u64) -> Msg) {
    let t = l.t;
    ui.with_keyed(
        key,
        NodeSpec::row()
            .pad(3.0)
            .gap(2.0)
            .radius(radius::CONTROL)
            .bg(t.sunken)
            .role(Role::Group)
            .label(key),
        |ui| {
            for (i, o) in options.iter().enumerate() {
                let on = i == current;
                let mut spec = NodeSpec::row()
                    .pad_xy(12.0, 6.0)
                    .radius(radius::CONTROL)
                    .on_click(msg(i as u64))
                    .role(Role::Checkbox)
                    .checked(on)
                    .label(*o);
                // The chosen one raised: white on the light well; on the
                // dark one, a step lighter than the card, or it would not
                // show.
                let raised = if t.appearance == kui_native::Appearance::Dark { t.border } else { l.card };
                spec = if on {
                    spec.bg(raised).shadow_color(l.shadow).shadow_blur(6.0).shadow_y(1.0)
                } else {
                    spec.hover_bg(t.hover)
                };
                ui.text_in_indexed(
                    i as u64,
                    spec,
                    o,
                    TextStyle::new(Type::UI.size).color(if on { t.fg } else { t.muted }).nowrap(),
                );
            }
        },
    );
}

fn step_button(ui: &mut Ui<'_>, l: &Look, key: &str, glyph: Icon, name: &str, enabled: bool, msg: Msg) {
    let t = l.t;
    ui.with_keyed(
        key,
        NodeSpec::row()
            .size(30.0, 26.0)
            .center()
            .radius(radius::CONTROL)
            .hover_bg(t.hover)
            .pressed_bg(t.pressed)
            .on_click(msg)
            .disabled(!enabled)
            .opacity(if enabled { 1.0 } else { design::DISABLED_OPACITY })
            .label(name),
        |ui| crate::icons::icon(ui, glyph, t.fg, 16.0, false),
    );
}

/// The face in use, named in itself, and a chevron that opens the list.
fn font_pill(ui: &mut Ui<'_>, l: &Look, key: &str, name: &str, face: FontFamily, open: bool, msg: Msg) {
    let t = l.t;
    ui.with_keyed(
        key,
        NodeSpec::row()
            .pad_xy(12.0, 6.0)
            .gap(8.0)
            .radius(radius::CONTROL)
            .bg(t.sunken)
            .hover_bg(t.sunken.mix(t.fg, 0.05))
            .cross_align(Align::Center)
            .on_click(msg)
            .role(Role::Checkbox)
            .checked(open)
            .label(format!("{name}, choose another font")),
        |ui| {
            ui.text(name, TextStyle::new(Type::STRONG.size).family(face).color(t.fg).nowrap());
            let chevron = if open { Icon::ChevronUp } else { Icon::ChevronDown };
            crate::icons::icon(ui, chevron, t.muted, 14.0, false);
        },
    );
}

/// A rounded button with a word: the accent's for the main one.
/// A pill with an icon before its word.
#[allow(clippy::too_many_arguments)]
fn icon_pill(ui: &mut Ui<'_>, l: &Look, key: &str, g: Icon, word: &str, main: bool, enabled: bool, msg: Msg) {
    let t = l.t;
    let (bg, ink) = if main { (t.focus_ring, t.on_accent) } else { (t.sunken, t.fg) };
    ui.with_keyed(
        key,
        NodeSpec::row()
            .pad_xy(12.0, 6.0)
            .gap(6.0)
            .radius(radius::CONTROL)
            .bg(bg)
            .hover_bg(if main { t.focus_ring.mix(Color::WHITE, 0.08) } else { t.sunken.mix(t.fg, 0.05) })
            .pressed_bg(if main { t.focus_ring.mix(Color::BLACK, 0.08) } else { t.sunken.mix(t.fg, 0.09) })
            .cross_align(Align::Center)
            .on_click(msg)
            .disabled(!enabled)
            .opacity(if enabled { 1.0 } else { design::DISABLED_OPACITY })
            .label(word),
        |ui| {
            crate::icons::icon(ui, g, ink, 15.0, false);
            ui.text(word, TextStyle::new(Type::UI.size).color(ink).nowrap());
        },
    );
}

/// How many days the server keeps what a snapshot sets aside (the
/// server's `hub::KEEP`, docs/SYNC.md §6).
const KEEP_DAYS: u64 = 30;

/// Bytes as people say them: "340 KB", "1.2 GB".
/// What's said of an account at or over its plan's limit, if it is: the
/// server takes no new picture then (notes' text it always takes:
/// docs/SYNC.md §11), so new pictures wait here.
pub(super) fn over_limit(kept: u64, limit: u64) -> Option<String> {
    let what = if kept > limit {
        format!("Over this plan's {}", size_words(limit))
    } else if kept == limit {
        format!("Reached this plan's {}", size_words(limit))
    } else {
        return None;
    };
    Some(format!("{what}: new pictures stay on this device until there's room. Notes still sync."))
}

/// How long something waits to be sent before the line says so: a wait
/// a fast server ends within a frame or two shows nothing.
pub(super) const WAIT_SHOWN_AFTER: f64 = 0.3;

/// How long "All sent" stays once what the line said was waiting has gone.
pub(super) const SENT_SHOWN_FOR: f64 = 1.5;

pub(super) const ALL_SENT: &str = "All sent";

/// What the waiting line says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Waited {
    Waiting,
    Sent,
}

/// Settings → Sync's "Waiting to send" line, kept from flashing: said
/// only once something has waited [`WAIT_SHOWN_AFTER`], and when all of
/// it has gone, "All sent" for [`SENT_SHOWN_FOR`], then nothing.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WaitLine {
    /// Since when something has waited, by the frame clock.
    since: Option<f64>,
    /// The line has said "Waiting" for this wait.
    shown: bool,
    /// When all that it said was waiting went.
    sent_at: Option<f64>,
}

impl WaitLine {
    /// What to say at `now`, with something waiting or not; and when a
    /// frame is wanted to say the next thing, should nothing else draw one.
    pub(super) fn said(&mut self, now: f64, waiting: bool) -> (Option<Waited>, Option<f64>) {
        let sent_until = self.sent_at.map(|at| at + SENT_SHOWN_FOR).filter(|&until| now < until);
        if waiting {
            let since = *self.since.get_or_insert(now);
            if self.shown || now >= since + WAIT_SHOWN_AFTER {
                self.shown = true;
                self.sent_at = None;
                return (Some(Waited::Waiting), None);
            }
            // Not yet: "All sent" stays while it has its moment.
            let next = since + WAIT_SHOWN_AFTER;
            return (sent_until.map(|_| Waited::Sent), Some(sent_until.map_or(next, |u| u.min(next))));
        }
        self.since = None;
        if std::mem::take(&mut self.shown) {
            self.sent_at = Some(now);
            return (Some(Waited::Sent), Some(now + SENT_SHOWN_FOR));
        }
        match sent_until {
            Some(until) => (Some(Waited::Sent), Some(until)),
            None => {
                self.sent_at = None;
                (None, None)
            }
        }
    }
}

/// Changes and pictures waiting to be sent, in words.
pub(super) fn waiting_words(changes: usize, pictures: usize) -> String {
    format!("Waiting to send {}", waiting_count(changes, pictures))
}

/// "2 changes", "1 picture", "2 changes and 1 picture" (none of both is
/// never asked).
pub(super) fn waiting_count(changes: usize, pictures: usize) -> String {
    let count = |n: usize, one: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {one}s")),
    };
    match (count(changes, "change"), count(pictures, "picture")) {
        (Some(c), Some(p)) => format!("{c} and {p}"),
        (c, p) => c.or(p).unwrap_or_default(),
    }
}

pub(super) fn size_words(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1000 {
        return format!("{n} bytes");
    }
    let mut v = n as f64 / 1000.0;
    let mut unit = 0;
    while v >= 1000.0 && unit + 1 < UNITS.len() {
        v /= 1000.0;
        unit += 1;
    }
    if v < 10.0 { format!("{v:.1} {}", UNITS[unit]) } else { format!("{v:.0} {}", UNITS[unit]) }
}

/// One of several ways to choose between: the whole row a button, its
/// icon, its name, and a line on it. A step off the card it sits on, in
/// a control's edge, so each reads as a thing to press; a further step
/// under the pointer, and another pressed.
fn choice_row(ui: &mut Ui<'_>, l: &Look, key: &str, g: Icon, name: &str, what: &str, msg: Msg) {
    let t = l.t;
    let step = |k: f32| l.card.mix(t.fg, k);
    ui.with_keyed(
        key,
        NodeSpec::row()
            .grow_width()
            .gap(12.0)
            .padding(Edges { l: 14.0, r: 14.0, t: 10.0, b: 10.0 })
            .radius(radius::CONTROL)
            .bg(step(0.03))
            .border(1.0, t.border_strong)
            .hover_bg(step(0.07))
            .pressed_bg(step(0.12))
            .cross_align(Align::Center)
            .on_click(msg)
            .role(Role::Button)
            .label(name),
        |ui| {
            crate::icons::icon(ui, g, t.fg, 18.0, false);
            ui.with(NodeSpec::column().grow_width().min_width(0.0).gap(2.0), |ui| {
                ui.text(name, TextStyle::new(Type::UI.size).color(t.fg));
                ui.text(what, TextStyle::new(Type::SMALL.size).color(t.muted));
            });
        },
    );
}

/// A button that keeps to itself: its word only, no well under it until
/// the pointer is on it.
fn quiet_button(ui: &mut Ui<'_>, l: &Look, key: &str, word: &str, msg: Msg) {
    let t = l.t;
    ui.text_in_keyed(
        key,
        NodeSpec::row()
            .pad_xy(6.0, 4.0)
            .radius(radius::CONTROL)
            .hover_bg(t.sunken.mix(t.fg, 0.05))
            .pressed_bg(t.sunken.mix(t.fg, 0.09))
            .on_click(msg)
            .role(Role::Button)
            .label(word),
        word,
        TextStyle::new(Type::SMALL.size).color(t.muted).nowrap(),
    );
}

fn pill_button(ui: &mut Ui<'_>, l: &Look, key: &str, word: &str, main: bool, enabled: bool, msg: Msg) {
    let t = l.t;
    // The main one on the accent deepened, so its word reads (4.5:1 and
    // up); the others on the sunken well, a step darker under the pointer.
    let (bg, ink) = if main { (t.focus_ring, t.on_accent) } else { (t.sunken, t.fg) };
    ui.text_in_keyed(
        key,
        NodeSpec::row()
            .pad_xy(14.0, 7.0)
            .radius(radius::CONTROL)
            .bg(bg)
            .hover_bg(if main { t.focus_ring.mix(Color::WHITE, 0.08) } else { t.sunken.mix(t.fg, 0.05) })
            .pressed_bg(if main { t.focus_ring.mix(Color::BLACK, 0.08) } else { t.sunken.mix(t.fg, 0.09) })
            .on_click(msg)
            .disabled(!enabled)
            .opacity(if enabled { 1.0 } else { design::DISABLED_OPACITY })
            .label(word),
        word,
        TextStyle::new(Type::UI.size).color(ink).nowrap(),
    );
}

/// `text` as a QR code's pixels, dark on white with its quiet zone, ten
/// pixels a module: sharp at the size Settings shows it on a Retina
/// screen. The side, and RGBA.
fn qr_rgba(text: &str) -> Option<(u32, Vec<u8>)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let (n, quiet, scale) = (code.width(), 4, 10);
    let side = (n + 2 * quiet) * scale;
    let colors = code.to_colors();
    let mut rgba = vec![255u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let (mx, my) = ((x / scale).wrapping_sub(quiet), (y / scale).wrapping_sub(quiet));
            if mx < n && my < n && colors[my * n + mx] == qrcode::Color::Dark {
                let at = (y * side + x) * 4;
                // The page's ink, near-black: as dark as a scanner wants.
                rgba[at..at + 3].copy_from_slice(&[0x1f, 0x1b, 0x18]);
            }
        }
    }
    Some((side as u32, rgba))
}

/// How long an invite has left, rounded up (one just made has a week):
/// "good for 7 more days", "good for 3 more hours".
fn left(now: u64, until: u64) -> String {
    let s = until.saturating_sub(now);
    let unit = |n: u64, one: &str| if n == 1 { format!("good for 1 more {one}") } else { format!("good for {n} more {one}s") };
    match s {
        0..3600 => unit(s.div_ceil(60).max(1), "minute"),
        3600..86_400 => unit(s.div_ceil(3600), "hour"),
        _ => unit(s.div_ceil(86_400), "day"),
    }
}

/// One thing the server logged, as "Lately" says it: its icon, whether
/// it warns, and its words (when, and by which device, follow). What
/// happened to a note is said with the note's name; what happened to the
/// account (a password, a passkey) is said as that, not as a note; a note
/// deleted forever is gone, so it is said as such.
pub(super) fn lately_words(store: &crate::store::Store, room: &str, what: &str) -> (Icon, bool, String) {
    let account = |g: Icon, words: &str| (g, false, words.to_string());
    match what {
        "set the password" => return account(Icon::Key, "Password set"),
        "took the password away" => return account(Icon::Key, "Password taken away"),
        "added a passkey" => return account(Icon::Key, "Passkey added"),
        "took a passkey away" => return account(Icon::Key, "Passkey taken away"),
        "deleted forever" => {
            // Gone here too, as a rule: named only if this device still
            // knows it.
            let known = carnet_sync::Key::parse(room).is_some_and(|k| store.notes.iter().any(|n| n.key == Some(k)) || store.replica.as_ref().is_some_and(|r| r.entry(k).is_some()));
            let words = if known { format!("\u{201c}{}\u{201d} deleted forever", sync_room_title(store, room)) } else { "A note deleted forever".into() };
            return (Icon::Trash, false, words);
        }
        _ => {}
    }
    if room.is_empty() {
        let mut c = what.chars();
        let words = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
        return (Icon::Clock, false, words);
    }
    let title = sync_room_title(store, room);
    let (g, warn, said) = if what == "rolled back" {
        (Icon::Undo, false, "packing undone".to_string())
    } else if what.starts_with("replaced") {
        (Icon::Upload, false, "replaced with a device's copy".to_string())
    } else if what.contains("did not hold") {
        let undone = what.ends_with("rolled back");
        (Icon::Warning, true, if undone { "a packed copy was wrong, and was undone".into() } else { "a packed copy was wrong".into() })
    } else {
        (Icon::Clock, false, what.to_string())
    };
    (g, warn, format!("{title}: {said}"))
}

/// What the inspector calls a room on the server (docs/SYNC.md §9): a
/// note by its title (one in Deleted by its file's name, and said so);
/// the vault as what it holds; and one this device has no copy of, as
/// such.
pub(super) fn sync_room_title(store: &crate::store::Store, room: &str) -> String {
    if room == carnet_sync::vault::ROOM {
        return "Folders and note names".into();
    }
    let Some(key) = carnet_sync::Key::parse(room) else { return "A note not on this device yet".into() };
    if let Some(n) = store.notes.iter().find(|n| n.key == Some(key)) {
        return n.display_title().to_string();
    }
    match store.replica.as_ref().and_then(|r| r.entry(key)) {
        Some(e) => {
            let name = e.path.rsplit('/').next().unwrap_or(&e.path);
            let name = name.strip_suffix(".md").unwrap_or(name);
            if e.path.starts_with(".trash/") { format!("{name} (in Deleted)") } else { name.to_string() }
        }
        None => "A note not on this device yet".into(),
    }
}
