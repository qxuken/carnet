//! A link to a file in the notes folder opens it in the system's own app
//! for it. One that could run a program as it opens (a program, an
//! installer, a script, a shortcut: `store::could_run`) is asked about
//! first, whoever put it there: the question names it, says what it could
//! do, and Cancel is what Enter, Esc or a press outside it does.

use super::*;

impl Carnet {
    /// A file a link leads to, opened: at once, or, one that could run,
    /// once the question before it is answered.
    pub(super) fn open_linked(&mut self, path: std::path::PathBuf) {
        if store::could_run(&path) {
            self.open_ask = Some(path);
        } else {
            self.open_file(&path);
        }
    }

    /// The file opened in its own app, or why it couldn't be.
    pub(super) fn open_file(&mut self, path: &std::path::Path) {
        if let Err(e) = store::open_file(path) {
            self.error = Some(format!("Couldn't open the file: {}", store::plain_error(&e)));
        }
    }

    /// The question before a file that could run opens, in the middle of
    /// the window: what it is, and Cancel or Open anyway.
    pub(super) fn open_question(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let Some(path) = self.open_ask.as_ref() else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let t = l.t;
        ui.with_keyed(
            "open-ask",
            NodeSpec::column()
                .float(FloatConfig::viewport().at(Align::Center, Align::Center).self_at(Align::Center, Align::Center))
                .width(400.0)
                .pad(16.0)
                .gap(10.0)
                .radius(radius::CARD)
                .bg(l.card)
                .border(1.0, t.border)
                .shadow_color(l.shadow)
                .shadow_blur(18.0)
                .shadow_y(4.0)
                .modal(Msg::OpenCancel)
                .role(Role::Dialog)
                .label(format!("Open \u{201c}{name}\u{201d}?")),
            |ui| {
                ui.text_in(
                    NodeSpec::row().grow_width().min_width(0.0),
                    &format!("Open \u{201c}{name}\u{201d}?"),
                    TextStyle::new(Type::STRONG.size).color(t.fg),
                );
                ui.text_in(NodeSpec::row().grow_width().min_width(0.0), OPEN_ASK_WHY, TextStyle::new(Type::SMALL.size).color(t.muted));
                ui.with(NodeSpec::row().grow_width().gap(6.0).cross_align(Align::Center), |ui| {
                    ui.leaf(NodeSpec::row().grow_width());
                    let button = |ui: &mut Ui<'_>, words: &str, msg: Msg, first: bool| {
                        let mut spec = NodeSpec::row()
                            .pad_xy(12.0, 6.0)
                            .radius(radius::CONTROL)
                            .bg(if first { l.c.primary } else { Color::TRANSPARENT })
                            .border(1.0, if first { l.c.primary } else { t.border })
                            .hover_bg(if first { l.c.primary } else { t.hover })
                            .on_click(msg)
                            .focusable()
                            .role(Role::Button)
                            .label(words);
                        if first {
                            spec = spec.initial_focus();
                        }
                        ui.text_in(spec, words, TextStyle::new(Type::SMALL.size).color(if first { l.c.on_accent } else { t.fg }).nowrap());
                    };
                    button(ui, "Open anyway", Msg::OpenAnyway, false);
                    button(ui, "Cancel", Msg::OpenCancel, true);
                });
            },
        );
    }
}

/// What the question before a file that could run says of it.
pub(super) const OPEN_ASK_WHY: &str = "It could run a program on this computer. Open it only if you know where it came from.";
