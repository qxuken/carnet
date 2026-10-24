//! The Mac's menu bar: every command the app has, where a Mac app keeps
//! them, each with its shortcut.
//!
//! On macOS a row's shortcut is a real key equivalent: AppKit matches it
//! before the window hears the key. So a row bound to a key does not run
//! a command of its own: it is kui's replay row, which plays its key back
//! to wherever the keyboard is (the page, the title or the search, or the
//! app's chords at the root), and does exactly what the key would have
//! done. The title's ⌥↑ is still the title's, and Undo in the search is
//! the search's, with nothing rebuilt as the keyboard moves. A row that
//! means nothing where the keyboard is — Bold in the search, Undo with
//! nothing to undo — is greyed, and a greyed row's key goes on to the
//! window as if the row were not there. The application menu's About,
//! Hide and Quit are AppKit's own.

use kui_native::{BarMenu, MenuBar, MenuRole, WindowCommand, WindowId};

use super::*;
use crate::keys;

impl Carnet {
    /// The bar as things stand: what is checked, what a row says, and what
    /// applies where the keyboard is.
    pub(super) fn menu_bar(&self, core: &Core) -> MenuBar {
        let focus = core.focus();
        let on_page = focus.is_some() && focus == self.page_key;
        let field = focus.filter(|k| self.fields.contains_key(k));
        // What the keyboard's text can do: the page's own history and
        // caret, or the focused field's.
        let (undo, redo) = match field {
            Some(k) => core.edit.history(k),
            None if on_page => {
                let (u, r) = self
                    .histories
                    .get(&self.store.notes[self.current].id)
                    .map_or((0, 0), History::depth);
                (u > 0, r > 0)
            }
            None => (false, false),
        };
        let selected = match field {
            Some(k) => core.edit.has_selection(k),
            None => on_page && !self.caret.is_empty(),
        };
        let typing = on_page || field.is_some();
        // A row whose shortcut is played back as that key.
        let key = |label: &str, spec: &str| MenuItem::new(label).accel(spec).replay();
        // A row the app runs itself.
        let act = |label: &str, what: &str| MenuItem::new(label).id(Act::App { what: what.into() });
        // The application's own row, worded with the name: AppKit would
        // take the process's, which is the binary's lower-case one.
        let role = |r: MenuRole, label: &str| MenuItem { label: label.into(), ..MenuItem::role(r) };
        let note = &self.store.notes[self.current];

        let app = BarMenu::new(
            "Carnet",
            vec![
                role(MenuRole::About, "About Carnet"),
                MenuItem::separator(),
                key("Settings\u{2026}", keys::SETTINGS),
                MenuItem::separator(),
                role(MenuRole::Hide, "Hide Carnet"),
                MenuItem::role(MenuRole::HideOthers),
                MenuItem::role(MenuRole::ShowAll),
                MenuItem::separator(),
                role(MenuRole::Quit, "Quit Carnet"),
            ],
        );
        let file = BarMenu::new(
            "File",
            vec![
                key("New Note", keys::NEW_NOTE),
                key("New Folder", keys::NEW_FOLDER),
                MenuItem::separator(),
                key("Save", keys::SAVE),
                act("Copy Note as Markdown", "copy-note"),
                act(reveal_label(), "reveal-note"),
                MenuItem::separator(),
                key(if note.pinned { "Unpin Note" } else { "Pin Note" }, keys::PIN),
                key("Move Note to Deleted", keys::TRASH),
                MenuItem::separator(),
                act("Close Window", "close").accel("mod+w"),
            ],
        );
        // A row for the note's blocks: greyed off the page.
        let page = |label: &str, spec: &str| key(label, spec).enabled(on_page);
        // A row for the table the caret is in: greyed elsewhere, so its
        // key (Option+Shift+↑, say) is the page's own there.
        let in_table = on_page && self.caret.head.slot.cell.is_some();
        let table = |label: &str, spec: &str| key(label, spec).enabled(in_table);
        let edit = BarMenu::new(
            "Edit",
            vec![
                key("Undo", keys::UNDO).enabled(undo),
                key("Redo", keys::REDO).enabled(redo),
                MenuItem::separator(),
                key("Cut", "mod+x").enabled(selected),
                key("Copy", "mod+c").enabled(selected),
                key("Paste", keys::PASTE).enabled(typing),
                key("Select All", "mod+a").enabled(typing),
                MenuItem::separator(),
                key("Find", keys::SEARCH),
                MenuItem::separator(),
                page("Link\u{2026}", keys::LINK),
                page("Duplicate Block", keys::DUPLICATE),
                page("Remove Block", keys::REMOVE),
            ],
        );
        let format = BarMenu::new(
            "Format",
            vec![
                page("Text", keys::TEXT),
                page("Heading 1", keys::H1),
                page("Heading 2", keys::H2),
                page("Heading 3", keys::H3),
                page("Checklist", keys::TODO),
                page("Bulleted List", keys::BULLETS),
                page("Numbered List", keys::NUMBERS),
                page("Quote", keys::QUOTE),
                page("Drop the First Letter", keys::INITIAL),
                MenuItem::separator(),
                page("Bold", keys::BOLD),
                page("Italic", keys::ITALIC),
                page("Strikethrough", keys::STRIKE),
                page("Underline", keys::UNDERLINE),
                page("Code", keys::CODE),
                page("Highlight", keys::HIGHLIGHT),
                page("Change Case", keys::CHANGE_CASE),
                MenuItem::separator(),
                page("Table", keys::TABLE),
                page("Divider", keys::DIVIDER),
                MenuItem::separator(),
                page("Move Block Up", keys::MOVE_UP),
                page("Move Block Down", keys::MOVE_DOWN),
                table("Move Row Up", keys::ROW_UP),
                table("Move Row Down", keys::ROW_DOWN),
                table("Move Column Left", keys::COL_LEFT),
                table("Move Column Right", keys::COL_RIGHT),
                page("Tick or Untick", keys::TICK),
            ],
        );
        let s = &self.settings;
        let view = BarMenu::new(
            "View",
            vec![
                key(if self.side_shown() { "Hide Notes" } else { "Show Notes" }, keys::SIDEBAR),
                key("Zen Mode", keys::ZEN).checked(self.zen),
                act("Note History", "history").checked(self.versions.is_some()),
                MenuItem::separator(),
                act("Focus", "focus").checked(s.focus),
                act("Typewriter", "typewriter").checked(s.typewriter),
                act("Toolbar Fades While Typing", "fade-tools").checked(s.fade_tools),
                MenuItem::separator(),
                key("Previous Note", keys::PREVIOUS_NOTE),
                key("Next Note", keys::NEXT_NOTE),
            ],
        );
        // Titled "Window", kui registers it as the platform's: AppKit adds
        // full screen, the tiling rows and the window list. Minimize and
        // Zoom it leaves to the menu's own rows.
        let window = BarMenu::new(
            "Window",
            vec![act("Minimize", "minimize").accel("mod+m"), act("Zoom", "zoom")],
        );
        MenuBar::new(vec![app, file, edit, format, view, window]).on_menu(Msg::Chose)
    }

    /// A row of the bar the app runs itself chosen. (A row that is its key
    /// posts nothing: kui plays the key.)
    pub(super) fn on_bar(&mut self, what: &str) {
        match what {
            "close" => self.window_cmd = Some(WindowCommand::Close(WindowId::MAIN)),
            "minimize" => self.window_cmd = Some(WindowCommand::Minimize(WindowId::MAIN)),
            "zoom" => self.window_cmd = Some(WindowCommand::ToggleMaximize(WindowId::MAIN)),
            "copy-note" => self.on_plain(Msg::CopyText { what: 1 }),
            "history" if self.versions.is_some() => self.close_history(),
            "history" => self.show_history(),
            "reveal-note" => {
                if let Some(p) = self.store.notes[self.current].path.clone()
                    && let Err(e) = store::reveal(&p)
                {
                    self.error = Some(format!("Couldn't open the folder: {e}"));
                }
            }
            "focus" => self.on_settings(Msg::SetFocus { to: u64::from(!self.settings.focus) }),
            "typewriter" => self.on_settings(Msg::SetTypewriter { to: u64::from(!self.settings.typewriter) }),
            "fade-tools" => self.on_settings(Msg::SetToolbar { to: u64::from(!self.settings.fade_tools) }),
            _ => {}
        }
    }
}
