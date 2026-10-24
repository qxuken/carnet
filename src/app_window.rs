//! A note in a window of its own: the toolbar and the page, and nothing of
//! the library's. kui opens a window the frame declares and closes it once
//! no frame does; the main window's frame declares one for each note in
//! `windows`, and every window's `view` and events come through `Carnet`.
//!
//! The app is one `Carnet`, whose editor fields are the main window's
//! between turns. A note window's turn trades its own [`Editor`] in for the
//! main window's and back out after (`in_window`). The store, the settings
//! and sync are the same in every window, so a note open in two windows is
//! one note: what is typed in either is in the other at once, its undo is
//! one history, and the other window's caret is carried with the words
//! when it is next drawn or answered (`page::follow`).
//!
//! A note window keeps to its note: what would show another note there
//! (the next note, ⌃Tab) does nothing, and what is the library's (a new
//! note, the search, Settings) is done in the main window, which comes
//! forward. Its note moved to Deleted, or gone in sync or another app, the
//! window closes, and a notice it had with an Undo goes to the main window.
//! Closing the main window quits, every note window with it, as before.

use kui_native::{WindowCommand, WindowConfig, WindowId, WindowLifecycle};

use super::*;

/// How tall a note window opens; its width is the page's and this much
/// beside it.
const WINDOW_H: f32 = 740.0;
const WINDOW_MARGIN: f32 = 48.0;

impl Carnet {
    /// Puts the editor in place by and takes up `ed`, another window's;
    /// the store, the settings and sync stay. The one coming in finds its
    /// note again by id, as notes come and go while it is put by, and its
    /// caret is carried through what was done to the note since it last
    /// saw it. One whose note has gone (the main window's: a note window
    /// closes with its note) shows the library's first.
    pub(super) fn swap_editor(&mut self, ed: &mut Editor) {
        let leaving = self.store.notes.get(self.current).map(|n| (n.id, n.doc.snapshot()));
        self.swap_fields(ed);
        let (note, seen) = match leaving {
            Some((id, blocks)) => (std::mem::replace(&mut ed.note, id), std::mem::replace(&mut ed.seen, blocks)),
            None => (ed.note, std::mem::take(&mut ed.seen)),
        };
        match self.store.notes.iter().position(|n| n.id == note) {
            Some(i) => {
                self.current = i;
                self.caret = page::follow(&seen, &self.store.notes[i].doc, self.caret);
            }
            None => {
                let last = self.store.notes.len().saturating_sub(1);
                self.current = self.order.first().copied().unwrap_or(0).min(last);
                self.caret = Caret::at(page::first(self.doc()));
                self.editing = false;
            }
        }
    }

    /// Runs `f` as note window `i`'s turn: its editor in place, and the
    /// main window's put back after. `f` may close the window, or others.
    pub(super) fn in_window<T>(&mut self, i: usize, f: impl FnOnce(&mut Self) -> T) -> T {
        let note = self.windows[i].note;
        let mut ed = std::mem::take(&mut self.windows[i].ed);
        ed.note = note;
        self.swap_editor(&mut ed);
        self.note_window = Some(note);
        let out = f(self);
        self.note_window = None;
        self.swap_editor(&mut ed);
        if let Some(w) = self.windows.iter_mut().find(|w| w.note == note) {
            w.ed = ed;
        }
        out
    }

    /// Note `id` in a window of its own, or its window brought forward if
    /// it has one. Only from the main window: a note window is its note's.
    pub(super) fn open_in_window(&mut self, id: u64) {
        if self.note_window.is_some() {
            return;
        }
        if self.windows.iter().any(|w| w.note == id) {
            self.raise = Some(id);
            return;
        }
        if let Some(note) = self.store.notes.iter().find(|n| n.id == id) {
            let ed = Editor::of(note);
            self.windows.push(NoteWindow { note: id, ed });
        }
    }

    /// Note `note`'s window closed: Mod+W, its close button, the OS. What
    /// was typed there is saved now rather than once typing would have
    /// paused.
    pub(super) fn close_window(&mut self, note: u64) {
        let Some(w) = self.windows.iter().position(|w| w.note == note) else { return };
        self.windows.remove(w);
        let Some(i) = self.store.notes.iter().position(|n| n.id == note) else { return };
        let n = &self.store.notes[i];
        // Nothing to keep yet, as for the main window's.
        if !n.dirty || (n.is_blank() && n.path.is_none()) {
            return;
        }
        match self.store.save(i, true) {
            Ok(Saved::Merged | Saved::Conflict(_)) => {
                self.histories.remove(&note);
                self.reorder();
            }
            Ok(_) => {}
            Err(e) => self.error = Some(format!("Couldn't save: {e}")),
        }
    }

    /// The windows whose note has gone — to Deleted, in sync, in another
    /// app — closed with it. A notice one had with an Undo ("Moved to
    /// Deleted · Undo") is said in the main window, where it can be taken.
    pub(super) fn prune_windows(&mut self) {
        let notes = &self.store.notes;
        let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.windows)
            .into_iter()
            .partition(|w| notes.iter().any(|n| n.id == w.note));
        self.windows = keep;
        for w in gone {
            if let Some(n) = w.ed.notice.filter(|n| n.undo.is_some()) {
                self.say = Some(n.text.clone());
                self.notice = Some(Notice { note: None, at: Instant::now(), ..n });
            }
        }
    }

    /// A chord in note window `note` that is not what it is in the main
    /// window: the library's keys (a new note or folder, the search,
    /// Settings) are the main window's, which comes forward to do them;
    /// the library shown or hidden and zen would change that window out of
    /// sight, and are nothing here; Mod+W closes this one. (The next note
    /// and ⌃Tab are nothing here too: `step_note`.) True when it was one.
    pub(super) fn note_window_chord(&mut self, note: u64, k: &kui_native::KeyPress) -> bool {
        if k.mods.alt {
            return false;
        }
        let to_main = match crate::keys::chord_code(k) {
            KeyCode::Char('n') if k.mods.shift => Msg::NewFolder,
            KeyCode::Char('n') => Msg::NewNote,
            KeyCode::Char(',') => Msg::ShowSettings,
            KeyCode::Char('f') => Msg::FocusSearch,
            KeyCode::Char('w') => {
                self.close_window(note);
                return true;
            }
            KeyCode::Char('\\' | '.') => return true,
            KeyCode::Char('o') if k.mods.shift => return true,
            _ => return false,
        };
        self.to_main = Some(to_main);
        true
    }

    /// Asked of a note window, done by the main window: its editor is the
    /// one in place again. Settings open, not shut, from there.
    pub(super) fn for_main(&mut self, msg: Msg) {
        match msg {
            Msg::ShowSettings if self.settings_open => {}
            Msg::FocusSearch => {
                self.list_view = true;
                self.settings_open = false;
                self.focus(Field::Search);
            }
            msg => self.on_plain(msg),
        }
    }

    /// The main window's frame declares every note window, as wide as the
    /// page and a margin as it opens; brings forward one asked for again;
    /// and draws again one whose note changed since it last drew it (here,
    /// in another app, in sync), which no input of its own would.
    pub(super) fn declare_windows(&mut self, ui: &mut Ui<'_>) {
        self.prune_windows();
        let size = (self.settings.width.px() + 2.0 * WINDOW_MARGIN, WINDOW_H);
        let open = ui.core().windows();
        let raise = self.raise.take();
        for w in &self.windows {
            let name = w.name();
            let config = WindowConfig::sized(size.0, size.1);
            ui.window_with(&name, config, WindowLifecycle::default().on_window(Msg::NoteWindow { note: w.note }));
            let Some(id) = open.iter().find(|(_, n)| **n == *name).map(|(id, _)| *id) else { continue };
            if raise == Some(w.note) {
                ui.core().focus_window(id);
            }
            let now = self.store.notes.iter().find(|n| n.id == w.note).map(|n| n.doc.blocks());
            let same = |b: &[Rc<Block>]| b.len() == w.ed.seen.len() && b.iter().zip(&w.ed.seen).all(|(a, b)| Rc::ptr_eq(a, b));
            if now.is_some_and(|b| !same(b)) {
                ui.window_command(WindowCommand::Redraw(id));
            }
        }
    }

    /// A note window's frame: its title the note's, then the toolbar and
    /// the page, as the main window draws them beside the library.
    pub(super) fn note_window_frame(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let title = self.store.notes[self.current].display_title().to_string();
        ui.window_title(&title);
        let vw = ui.viewport().w;
        ui.configure_root(
            NodeSpec::column()
                .fill()
                .bg(l.t.bg)
                .on_key(Msg::RootKey)
                .on_drop(Msg::FilesOver),
        );
        ui.window_events(WindowEvents::default().on_focus(Msg::WindowFocus));
        self.follow_file_drag(ui);
        self.strip(ui, l, false);
        self.page(ui, l, vw);
        // The words scrolling up under the strip fade, as in the main
        // window.
        ui.leaf(
            NodeSpec::row()
                .float(FloatConfig::parent().offset(0.0, STRIP_H))
                .size(vw - 12.0, 22.0)
                .gradient(kui_native::Gradient::to(kui_native::Side::Bottom, [l.t.bg, l.t.bg.with_alpha(0.0)]))
                .role(Role::None),
        );
        self.tint_picker(ui, l);
        self.link_editor(ui, l);
        self.open_question(ui, l);
    }

    /// An event in a note window, answered as that window's; what it asked
    /// of the main window done there after, and the main window brought
    /// forward. One from a window that is closing is not answered.
    pub(super) fn note_window_event(&mut self, ev: UiEvent, core: &mut Core) {
        let name = core.window_name();
        let Some(i) = self.windows.iter().position(|w| *w.name() == *name) else { return };
        self.in_window(i, |app| app.on_window_event(ev, core));
        self.prune_windows();
        if let Some(msg) = self.to_main.take() {
            self.for_main(msg);
            core.focus_window(WindowId::MAIN);
        }
    }
}
