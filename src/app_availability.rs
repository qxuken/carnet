//! Cloud placeholders have a view, never an editable document.
use super::*;
use std::path::PathBuf;

impl Carnet {
    fn synced_copy_ready(&self) -> bool {
        matches!(self.sync.status, crate::sync::Status::Online) && self.sync.caught_up()
    }

    pub(super) fn recover_synced_copy(&mut self) {
        if !self.synced_copy_ready() { return; }
        let Some(path) = self.unavailable_target() else { return };
        match self.store.recover_synced_copy(&path) {
            Ok(i) => {
                let id = self.store.notes[i].id;
                self.reorder();
                if let Some(i) = self.store.notes.iter().position(|n| n.id == id) { self.open(i); }
                self.notify("Recovered a separate copy. The original is unchanged.", None);
            }
            Err(e) => self.download.error = Some(format!("Couldn't recover the synced copy: {e}")),
        }
    }

    pub(super) fn unavailable_target(&self) -> Option<PathBuf> {
        if self.peek.is_some() || self.versions.is_some() { return None; }
        if let Some(path) = &self.unavailable_open { return Some(path.clone()); }
        let note = self.store.notes.get(self.current)?;
        if !note.dirty && let Some(path) = &note.path && self.store.is_unavailable(path) {
            return Some(path.clone());
        }
        None
    }

    pub(super) fn unavailable_rows(&self) -> Vec<(PathBuf, String)> {
        let query = self.query.trim().to_lowercase();
        self.store.unavailable_entries().into_iter().filter(|(p, title)| {
            if !query.is_empty() { return title.to_lowercase().contains(&query); }
            self.folder.as_ref().is_none_or(|f| p.parent() == Some(self.store.folder_dir(f).as_path()) || p == &self.store.folder_dir(f))
        }).collect()
    }

    pub(super) fn open_unavailable(&mut self, path: PathBuf) {
        self.leave_current();
        self.close_history();
        self.unavailable_open = Some(path);
        self.editing = false;
        self.focus_page = false;
        self.blur = true;
        self.list_view = false;
        self.settings_open = false;
        self.picked.clear();
        self.cards_clicked = false;
        self.folders_clicked = false;
    }

    pub(super) fn availability_tick(&mut self, ui: &mut Ui<'_>) {
        if let Some(root) = self.download.poll() && root == self.store.dir {
            self.rescan();
            self.retry_deleted_download();
            self.images.retry_failed();
        }
        if self.download.busy() {
            ui.request_frame_at(ui.now() + 0.25);
        }
    }

    pub(super) fn retry_availability(&mut self) {
        self.download.error = None;
        self.rescan();
        self.retry_deleted_download();
        self.images.retry_failed();
    }

    fn retry_deleted_download(&mut self) {
        if let Some(path) = self.unavailable_open.clone().filter(|p| p.starts_with(self.store.dir.join(".trash"))) {
            self.refresh_trash();
            if let Some(i) = self.trash.iter().position(|t| t.path == path) { self.open_deleted(i); }
        }
    }

    pub(super) fn availability_banner(&self, ui: &mut Ui<'_>, l: &Look) {
        if self.store.unavailable.is_empty() && !self.download.busy() && self.download.error.is_none() { return; }
        ui.with_keyed("availability-banner", NodeSpec::column().grow_width().pad_xy(26.0, 12.0).gap(6.0), |ui| {
            let text = if self.store.incomplete.contains(&self.store.dir) {
                "Your notes folder isn't available on this Mac."
            } else { "Some notes aren't available on this Mac." };
            ui.text(text, TextStyle::new(Type::SMALL.size).color(l.t.fg));
            let status = self.download.status();
            if !status.is_empty() { ui.text(&status, TextStyle::new(Type::SMALL.size).color(l.t.muted)); }
            ui.with(NodeSpec::row().wrap().gap(4.0), |ui| {
                if self.download.busy() {
                    Self::availability_action(ui, l, "cancel-vault-download", "Cancel download", Msg::CancelDownload);
                } else {
                    Self::availability_action(ui, l, "download-vault", "Download all", Msg::DownloadVault);
                }
                Self::availability_action(ui, l, "retry-availability", "Retry", Msg::RetryAvailability);
                Self::availability_action(ui, l, "reveal-availability", reveal_label(), Msg::ShowDir);
            });
        });
    }

    pub(super) fn unavailable_page(&mut self, ui: &mut Ui<'_>, l: &Look, path: &std::path::Path) {
        self.page_key = None;
        self.page_rect = None;
        self.row_rects.clear();
        self.slot_keys.clear();
        self.editing = false;
        let synced_copy = self.synced_copy_ready().then(|| self.store.recovery_copy(path)).flatten();
        ui.with_keyed("unavailable-page", NodeSpec::column().grow_width().grow_height().scroll_y().pad_xy(36.0, 36.0).gap(14.0), |ui| {
            crate::icons::icon(ui, crate::icons::Glyph::Cloud, l.t.muted, 32.0, false);
            let title = self.store.unavailable_entries().into_iter().find(|(p, _)| p == path)
                .map(|(_, title)| title).unwrap_or_else(|| "Notes unavailable".into());
            ui.text(&title, TextStyle::new(Type::HEADING.size).family(l.serif).color(l.t.fg));
            ui.text("This file or folder isn't available to read. If it's stored in iCloud, download it to continue. Otherwise, check its location and permissions.", TextStyle::new(Type::UI.size).color(l.t.muted));
            let status = self.download.status();
            if !status.is_empty() { ui.text(&status, TextStyle::new(Type::SMALL.size).color(l.t.fg)); }
            ui.with(NodeSpec::row().wrap().gap(4.0), |ui| {
                if self.download.busy() {
                    Self::availability_action(ui, l, "cancel-download", "Cancel download", Msg::CancelDownload);
                } else {
                    Self::availability_action(ui, l, "download-unavailable", "Download", Msg::DownloadUnavailable);
                }
                Self::availability_action(ui, l, "retry-unavailable", "Retry", Msg::RetryAvailability);
                Self::availability_action(ui, l, "reveal-unavailable", reveal_label(), Msg::RevealUnavailable);
            });
            if let Some((_, modified)) = &synced_copy {
                let when = if let Some(time) = u64::try_from(*modified).ok().filter(|s| *s > 0).and_then(store::epoch_time) {
                    let secs = *modified as u64;
                    let date = chrome::date_words(time);
                    format!("modified {date} at {:02}:{:02} UTC", secs / 3600 % 24, secs / 60 % 60)
                } else { "modification time unknown".into() };
                ui.text(&format!("Synced copy available · {when}"), TextStyle::new(Type::SMALL.size).color(l.t.fg));
                ui.text("Create a separate note from the synced copy. The iCloud original may contain newer edits. Attachments may still need downloading.", TextStyle::new(Type::SMALL.size).color(l.t.muted));
                Self::availability_action(ui, l, "recover-synced-copy", "Recover synced copy", Msg::RecoverSyncedCopy);
            }
            #[cfg(target_os = "macos")]
            ui.text("To keep your notes available offline, select the Carnet folder in Finder and choose Keep Downloaded.", TextStyle::new(Type::SMALL.size).color(l.t.muted));
        });
    }

    fn availability_action(ui: &mut Ui<'_>, l: &Look, key: &str, label: &str, msg: Msg) {
        ui.text_in_keyed(key, NodeSpec::row().pad_xy(8.0, 5.0).radius(6.0).hover_bg(l.t.accent_soft)
            .on_click(msg).label(label).role(Role::Button), label, TextStyle::new(Type::SMALL.size).color(l.t.focus_ring));
    }
}
