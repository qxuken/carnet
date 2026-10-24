//! The app driven headlessly through the real `view` and `on_event`:
//! typed text, keys, clicks, drags — no window, no GPU.

use super::*;
use kui_native::testing::Drive;
use kui_native::{InputEvent, KeyMods};

fn temp_store(name: &str) -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("carnet-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Store::open(dir.clone()).unwrap(), dir)
}

/// A fresh app on a new, empty note, its title focused (the welcome note
/// stays behind it).
fn fresh(name: &str) -> (Carnet, Drive, std::path::PathBuf) {
    let (store, dir) = temp_store(name);
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.frame(&mut app);
    app.new_note();
    d.frame(&mut app);
    d.frame(&mut app);
    (app, d, dir)
}

/// A fresh note with the caret in its body.
fn body(name: &str) -> (Carnet, Drive, std::path::PathBuf) {
    let (mut app, mut d, dir) = fresh(name);
    key(&mut app, &mut d, "enter");
    (app, d, dir)
}

fn key(app: &mut Carnet, d: &mut Drive, name: &str) {
    d.key(app, name, KeyMods::NONE);
    d.frame(app);
}

fn key_with(app: &mut Carnet, d: &mut Drive, name: &str, mods: KeyMods) {
    d.key(app, name, mods);
    d.frame(app);
}

const SHIFT: KeyMods = KeyMods {
    shift: true,
    ctrl: false,
    alt: false,
    super_key: false,
};
const CTRL: KeyMods = KeyMods {
    shift: false,
    ctrl: true,
    alt: false,
    super_key: false,
};

/// Text as the OS hands a committed string to the focused sink.
fn type_in(app: &mut Carnet, d: &mut Drive, s: &str) {
    d.commit(app, s);
    d.frame(app);
}

fn blocks(app: &Carnet) -> Vec<Rc<Block>> {
    app.doc().blocks().to_vec()
}

fn texts(app: &Carnet) -> Vec<String> {
    app.doc()
        .blocks()
        .iter()
        .map(|b| match &b.kind {
            Kind::Item { text, done, .. } => format!("[{}]{text}", if *done { "x" } else { " " }),
            Kind::Table(_) => "<table>".into(),
            Kind::Divider => "---".into(),
            _ => b.text_ref().unwrap_or("").to_string(),
        })
        .collect()
}

fn copied(d: &mut Drive) -> Option<String> {
    d.core.take_menu_actions().into_iter().find_map(|a| match a {
        kui_native::MenuAction::SetClipboard { text, .. } => Some(text),
        _ => None,
    })
}

fn page_focused(app: &Carnet, d: &Drive) -> bool {
    d.core.focus().is_some() && d.core.focus() == app.page_key
}

use std::rc::Rc;
use carnet_core::doc::Float;

#[test]
fn first_frame_is_clean_and_the_title_has_the_caret() {
    let (mut app, mut d, dir) = fresh("clean");
    d.frame(&mut app);
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let focused = d.core.focus().and_then(|k| app.fields.get(&k)).copied();
    assert_eq!(focused, Some(Field::Title));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_checklist_typed_from_the_keyboard() {
    let (mut app, mut d, dir) = fresh("list");
    d.text(&mut app, "Groceries");
    d.frame(&mut app);
    // Enter in the title: into the body.
    key(&mut app, &mut d, "enter");
    assert!(page_focused(&app, &d));
    type_in(&mut app, &mut d, "[] ");
    assert!(matches!(blocks(&app)[0].kind, Kind::Item { style: ListStyle::Todo, .. }));
    type_in(&mut app, &mut d, "milk");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "eggs");
    key(&mut app, &mut d, "enter");
    // Enter on the empty third item steps out of the list.
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[..3], ["[ ]milk", "[ ]eggs", ""]);

    // A click on the box ticks it; Ctrl+Z unticks it again.
    d.core.set_inspect(true);
    d.frame(&mut app);
    let check = d
        .core
        .nodes()
        .iter()
        .find(|n| n.label.as_deref() == Some("check"))
        .map(|n| n.key)
        .expect("a checkbox");
    d.click_key(&mut app, check);
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "[x]milk");
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(texts(&app)[0], "[ ]milk", "undo takes the tick back");
    key_with(&mut app, &mut d, "y", CTRL);
    assert_eq!(texts(&app)[0], "[x]milk", "and redo puts it back");

    app.save_current();
    let path = app.store.notes[app.current].path.clone().unwrap();
    assert_eq!(path.file_name().unwrap(), "Groceries.md");
    let md = std::fs::read_to_string(path).unwrap();
    assert!(md.starts_with("# Groceries\n\n- [x] milk\n- [ ] eggs\n"), "{md}");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn enter_at_the_start_of_a_checked_item_leaves_it_checked() {
    let (mut app, mut d, dir) = body("enter-start");
    type_in(&mut app, &mut d, "[x] milk");
    assert_eq!(texts(&app)[0], "[x]milk");
    key(&mut app, &mut d, "home");
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[..2], ["[ ]", "[x]milk"]);
    assert_eq!(page::text(app.doc(), app.caret.head.slot), "milk");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn arrows_cross_blocks_and_table_cells() {
    let (mut app, mut d, dir) = body("arrows");
    type_in(&mut app, &mut d, "Intro");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "/table");
    key(&mut app, &mut d, "enter");
    let table = blocks(&app)[1].id;
    let Kind::Table(t) = &blocks(&app)[1].kind else { panic!("{:?}", texts(&app)) };
    let (rows, cols) = (t.rows.iter().map(|r| r.id).collect::<Vec<_>>(), t.cols.clone());
    let cell = |r: usize, c: usize| Slot {
        block: table,
        cell: Some((rows[r], cols[c])),
    };
    assert_eq!(app.caret.head.slot, cell(0, 0));
    type_in(&mut app, &mut d, "Name");
    // Tab walks the row; Down goes down the column.
    key(&mut app, &mut d, "tab");
    assert_eq!(app.caret.head.slot, cell(0, 1));
    key(&mut app, &mut d, "down");
    assert_eq!(app.caret.head.slot, cell(1, 1));
    key(&mut app, &mut d, "down");
    key(&mut app, &mut d, "down");
    // Past the last row: out of the table, into the paragraph below.
    assert_eq!(app.caret.head.slot, Slot::block(blocks(&app)[2].id));
    // Up comes back into the table's last row; up and up again, out to
    // the paragraph above it.
    key(&mut app, &mut d, "up");
    assert_eq!(app.caret.head.slot.cell.map(|c| c.0), Some(rows[2]));
    for _ in 0..3 {
        key(&mut app, &mut d, "up");
    }
    assert_eq!(app.caret.head.slot, Slot::block(blocks(&app)[0].id));
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn backspace_takes_empty_lines_and_then_a_table_and_undo_brings_it_back() {
    let (mut app, mut d, dir) = body("delete");
    type_in(&mut app, &mut d, "one");
    key(&mut app, &mut d, "enter");
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[..3], ["one", "", ""]);
    // An empty line goes with one Backspace.
    key(&mut app, &mut d, "backspace");
    assert_eq!(texts(&app)[..2], ["one", ""]);

    let ins = d.key_of("t-table").expect("table tool");
    d.click_key(&mut app, ins);
    d.frame(&mut app);
    assert_eq!(texts(&app)[..3], ["one", "<table>", ""]);
    // Below the table: Down twice from the first cell lands under it.
    for _ in 0..3 {
        key(&mut app, &mut d, "down");
    }
    assert_eq!(app.caret.head.slot, Slot::block(blocks(&app)[2].id));
    key(&mut app, &mut d, "backspace");
    key(&mut app, &mut d, "backspace");
    assert_eq!(texts(&app)[..2], ["one", ""], "two Backspaces take a table");
    key_with(&mut app, &mut d, "z", CTRL);
    assert!(texts(&app).contains(&"<table>".to_string()), "and Ctrl+Z brings it back");
    let _ = std::fs::remove_dir_all(dir);
}

/// A table's row moves with Alt+Shift+↑↓ and its column with
/// Ctrl+Alt+Shift+←→ (Alt+Shift+←→ still selects by word in a cell), or
/// from the toolbar and the page's menu, which grey a move past the
/// table's edge or the header; the caret goes with its words, and each
/// move is one undo.
#[test]
fn a_tables_rows_and_columns_move_and_the_caret_with_them() {
    // Wide, so the table's tools all show in the strip.
    let (store, dir) = temp_store("table-move");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1800.0, 760.0).framing();
    d.frame(&mut app);
    app.new_note();
    d.frame(&mut app);
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    let table = Table::from_cells(vec![
        vec!["Day".into(), "Miles".into()],
        vec!["Mon".into(), "12".into()],
        vec!["Tue".into(), "30".into()],
    ]);
    let (top, mon, day, miles) = (table.rows[0].id, table.rows[1].id, table.cols[0], table.cols[1]);
    app.store.notes[app.current].doc = Doc::new(vec![Block::new(Kind::Table(table)), Block::text("")]);
    let slot = Slot { block: blocks(&app)[0].id, cell: Some((mon, day)) };
    app.caret = Caret::at(Pos { slot, byte: 2 });
    d.frame(&mut app);
    d.frame(&mut app);
    let words = |app: &Carnet| {
        let Kind::Table(t) = &blocks(app)[0].kind else { panic!() };
        t.rows.iter().map(|r| r.cells.join(" ")).collect::<Vec<_>>()
    };
    let alt_shift = KeyMods { shift: true, ctrl: false, alt: true, super_key: false };
    d.core.set_inspect(true);
    let enabled = |d: &mut Drive, key: &str| {
        let k = d.key_of(key).expect(key);
        d.core.nodes().iter().any(|n| n.key == k && !n.flags.contains(&"disabled"))
    };
    let menu = |app: &Carnet, label: &str| app.page_menu().into_iter().find(|m| m.label == label).expect(label).enabled;
    // Under the header, at the left: only down and right.
    assert_eq!(app.table_moves(), [false, true, false, true]);
    assert!(!enabled(&mut d, "t-rowup") && enabled(&mut d, "t-rowdown"));
    assert!(!enabled(&mut d, "t-colleft") && enabled(&mut d, "t-colright"));
    assert!(!menu(&app, "Move row up") && menu(&app, "Move column right"));
    key_with(&mut app, &mut d, "down", alt_shift);
    assert_eq!(words(&app), ["Day Miles", "Tue 30", "Mon 12"]);
    assert_eq!(app.caret, Caret::at(Pos { slot, byte: 2 }), "the caret in its words");
    assert!(enabled(&mut d, "t-rowup") && !enabled(&mut d, "t-rowdown"), "the bottom row");
    // Alt+Shift+→ selects to the word's end, the columns where they were.
    key_with(&mut app, &mut d, "right", alt_shift);
    assert_eq!(words(&app), ["Day Miles", "Tue 30", "Mon 12"]);
    assert_eq!((app.caret.anchor.byte, app.caret.head.byte), (2, 3), "the word's end selected");
    app.caret = Caret::at(Pos { slot, byte: 2 });
    let ctrl_alt_shift = KeyMods { shift: true, ctrl: true, alt: true, super_key: false };
    key_with(&mut app, &mut d, "right", ctrl_alt_shift);
    assert_eq!(words(&app), ["Miles Day", "30 Tue", "12 Mon"]);
    assert_eq!(app.caret, Caret::at(Pos { slot, byte: 2 }));
    // From the toolbar and the menu.
    let up = d.key_of("t-rowup").unwrap();
    d.click_key(&mut app, up);
    d.frame(&mut app);
    assert_eq!(words(&app), ["Miles Day", "12 Mon", "30 Tue"]);
    app.page_action("colleft");
    d.frame(&mut app);
    assert_eq!(words(&app), ["Day Miles", "Mon 12", "Tue 30"]);
    // Up again from under the header: nothing, the header stays on top.
    key_with(&mut app, &mut d, "up", alt_shift);
    assert_eq!(words(&app)[0], "Day Miles");
    // From the header, the row doesn't go down; its column does go.
    app.caret = Caret::at(Pos { slot: Slot { cell: Some((top, miles)), ..slot }, byte: 0 });
    d.frame(&mut app);
    assert_eq!(app.table_moves(), [false, false, true, false]);
    key_with(&mut app, &mut d, "down", alt_shift);
    assert_eq!(words(&app)[0], "Day Miles", "the header stays the header");
    // Each move one undo.
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(words(&app), ["Miles Day", "12 Mon", "30 Tue"]);
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(words(&app), ["Miles Day", "30 Tue", "12 Mon"]);
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_selection_runs_across_blocks_and_copies_as_markdown() {
    let (mut app, mut d, dir) = body("select");
    type_in(&mut app, &mut d, "- one");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "two");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "three");
    // From the end of "three", Shift+Up twice and Shift+Home: all three.
    key_with(&mut app, &mut d, "up", SHIFT);
    key_with(&mut app, &mut d, "up", SHIFT);
    key_with(&mut app, &mut d, "home", SHIFT);
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some("- one\n- two\n- three\n"));
    // Typing replaces the whole selection.
    type_in(&mut app, &mut d, "x");
    // The three items are one again, holding what was typed.
    assert_eq!(texts(&app)[0], "[ ]x");
    assert_eq!(app.doc().len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

/// A checklist item's words chosen whole copy with the box, so a paste
/// elsewhere is a checklist item again; a numbered one with its number.
#[test]
fn a_whole_checklist_item_copies_as_one() {
    let (mut app, mut d, dir) = body("copy-item");
    type_in(&mut app, &mut d, "[] milk");
    key_with(&mut app, &mut d, "home", SHIFT);
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some("- [ ] milk\n"));
    // Ticked, and chosen with Ctrl+A (the block's text): the tick too.
    key_with(&mut app, &mut d, "enter", CTRL);
    key_with(&mut app, &mut d, "a", CTRL);
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some("- [x] milk\n"));
    // A word of it: the word.
    key(&mut app, &mut d, "end");
    key_with(&mut app, &mut d, "left", SHIFT);
    key_with(&mut app, &mut d, "left", SHIFT);
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some("lk"));
    // Pasted on an empty line: a checklist item.
    key(&mut app, &mut d, "end");
    key(&mut app, &mut d, "enter");
    key(&mut app, &mut d, "enter");
    app.edit(Step::Other, |doc, c| page::paste(doc, c, "- [x] milk\n"));
    assert_eq!(texts(&app)[..2], ["[x]milk", "[x]milk"], "{:?}", texts(&app));
    // A numbered item, with its number.
    let (mut app, mut d, dir2) = body("copy-number");
    type_in(&mut app, &mut d, "1. first");
    key_with(&mut app, &mut d, "home", SHIFT);
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some("1. first\n"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}

#[test]
fn a_drag_in_the_margin_selects_blocks() {
    let (mut app, mut d, dir) = body("margin");
    for (i, s) in ["- one", "two", "three"].iter().enumerate() {
        if i > 0 {
            key(&mut app, &mut d, "enter");
        }
        type_in(&mut app, &mut d, s);
    }
    d.frame(&mut app);
    let rects = app.row_rects.clone();
    let (r0, r2) = (rects[0].1, rects[2].1);
    d.drag(
        &mut app,
        Vec2::new(r0.x - 20.0, r0.y + 6.0),
        Vec2::new(r2.x - 20.0, r2.y + 6.0),
    );
    d.frame(&mut app);
    assert_eq!(page::touched(app.doc(), app.caret), 0..=2);
    key(&mut app, &mut d, "backspace");
    assert_eq!(texts(&app)[0], "");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_caret_is_drawn_where_the_text_ends() {
    let (mut app, mut d, dir) = body("caret");
    type_in(&mut app, &mut d, "hello");
    d.frame(&mut app);
    d.core.set_inspect(true);
    d.frame(&mut app);
    let slot = app.caret.head.slot;
    let k = *app.slot_keys.get(&slot).expect("the slot's text node");
    let r = d.core.caret_rect(k, 5).expect("a caret rect for the end");
    let o = d.core.caret_rect(k, 0).expect("and for the start");
    assert!(r.x > o.x + 20.0, "five letters in: {r:?} vs {o:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The caret of a line scrolled up past the page's top is cut there, as
/// its words are: never drawn over the toolbar.
#[test]
fn a_caret_scrolled_past_the_page_is_cut_with_its_line() {
    let (mut app, mut d, dir) = body("caret-cut");
    type_in(&mut app, &mut d, "The first line");
    for i in 0..40 {
        key(&mut app, &mut d, "enter");
        type_in(&mut app, &mut d, &format!("Line {i}"));
    }
    key_with(&mut app, &mut d, "up", CTRL);
    key(&mut app, &mut d, "end");
    assert_eq!(app.doc().get(app.caret.head.slot.block).and_then(|b| b.text_ref()), Some("The first line"));
    d.core.set_inspect(true);
    // The wheel, a little at a time, until the caret's line is half
    // under the page's top.
    let page = app.page_rect.expect("the page");
    let mut bar = None;
    for _ in 0..200 {
        d.wheel(&mut app, page.x + page.w / 2.0, page.y + page.h / 2.0, 0.0, -4.0);
        d.frame(&mut app);
        let r = d.key_of("caret").and_then(|k| d.rect_of(k));
        if r.is_some_and(|r| r.y < page.y && r.y + r.h > page.y) {
            bar = r;
            break;
        }
    }
    let bar = bar.expect("the caret half under the page's top");
    // Its quad, in physical pixels, and what clips it.
    let s = d.core.scale();
    let (list, _) = d.core.output();
    let quad = list
        .quads
        .iter()
        .find(|q| (q.rect.x - bar.x * s).abs() < 1.0 && (q.rect.y - bar.y * s).abs() < 1.0 && (q.rect.w - bar.w * s).abs() < 1.0)
        .expect("the caret's quad");
    let clip = list.clips[quad.clip as usize].rect;
    assert!(clip.y >= page.y * s - 1.0, "cut at the page's top {}: {clip:?}", page.y * s);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn search_with_no_match_makes_the_note_and_then_clears() {
    let (store, dir) = temp_store("search");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.frame(&mut app);
    d.frame(&mut app);
    let k = *app.fields.iter().find(|(_, f)| **f == Field::Search).unwrap().0;
    d.focus(&mut app, k);
    d.text(&mut app, "Packing list");
    d.frame(&mut app);
    assert!(app.order.is_empty());
    key(&mut app, &mut d, "enter");
    d.frame(&mut app);
    assert_eq!(app.store.notes[app.current].title, "Packing list");
    let k = *app.fields.iter().find(|(_, f)| **f == Field::Search).unwrap().0;
    assert_eq!(d.core.edit_text(k).as_deref(), Some(""));
    let _ = std::fs::remove_dir_all(dir);
}

/// Esc in the search clears what was typed, and the whole list is back;
/// a second Esc leaves the field.
#[test]
fn esc_clears_the_search_then_leaves_it() {
    let (store, dir) = temp_store("search-esc");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.frame(&mut app);
    d.frame(&mut app);
    let all = app.order.len();
    assert!(all > 0);
    let k = *app.fields.iter().find(|(_, f)| **f == Field::Search).unwrap().0;
    d.focus(&mut app, k);
    d.text(&mut app, "zzz nothing like it");
    d.frame(&mut app);
    assert!(app.order.is_empty());
    key(&mut app, &mut d, "escape");
    d.frame(&mut app);
    assert_eq!(app.query, "");
    assert_eq!(d.core.edit_text(k).as_deref(), Some(""));
    assert_eq!(app.order.len(), all, "the whole list is back");
    assert_eq!(d.core.focus(), Some(k), "the keyboard stays in the search");
    key(&mut app, &mut d, "escape");
    d.frame(&mut app);
    assert_ne!(d.core.focus(), Some(k), "a second Esc leaves it");
    let _ = std::fs::remove_dir_all(dir);
}

/// "Lately" in Settings → Sync names what happened to the account as
/// that (a password set, a passkey added), not as a note this device
/// lacks; a note deleted forever as such; a note's own events by its name.
#[test]
fn lately_names_account_events_as_such() {
    use super::settings_view::lately_words;
    let (store, dir) = temp_store("lately");
    let welcome = store.notes[0].key.expect("the welcome note has a key").to_string();
    let words = |room: &str, what: &str| lately_words(&store, room, what);
    let (g, warn, w) = words("", "set the password");
    assert!(matches!(g, crate::icons::Glyph::Key) && !warn);
    assert_eq!(w, "Password set");
    assert_eq!(words("", "took the password away").2, "Password taken away");
    assert_eq!(words("", "added a passkey").2, "Passkey added");
    assert_eq!(words("", "took a passkey away").2, "Passkey taken away");
    let (g, _, w) = words(&carnet_sync::Key::random().to_string(), "deleted forever");
    assert!(matches!(g, crate::icons::Glyph::Trash));
    assert_eq!(w, "A note deleted forever");
    assert_eq!(words(&welcome, "deleted forever").2, "\u{201c}Welcome to Carnet\u{201d} deleted forever");
    assert_eq!(words(&welcome, "rolled back").2, "Welcome to Carnet: packing undone");
    let (_, warn, w) = words(&welcome, "a snapshot by device 00 did not hold what it said: rolled back");
    assert!(warn);
    assert_eq!(w, "Welcome to Carnet: a packed copy was wrong, and was undone");
    for (room, what) in [("", "set the password"), ("", "added a passkey"), (&*carnet_sync::Key::random().to_string(), "deleted forever")] {
        assert!(!words(room, what).2.contains("not on this device"), "{what}: said as a note");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_empty_fields_say_what_they_are_for() {
    let (store, dir) = temp_store("placeholder");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    app.new_note();
    d.frame(&mut app);
    d.frame(&mut app);
    let field = |app: &Carnet, f: Field| *app.fields.iter().find(|(_, g)| **g == f).unwrap().0;
    let (search, title) = (field(&app, Field::Search), field(&app, Field::Title));
    assert_eq!(d.core.edit.placeholder(search), Some("Search"));
    assert_eq!(d.core.edit.placeholder(title), Some("Untitled"));
    // The editors' own: no word floated over them. (A blank note's card
    // still says "Untitled".)
    for (k, word) in [(search, "Search"), (title, "Untitled")] {
        let r = d.core.nodes().iter().find(|n| n.key == k).map(|n| n.rect).unwrap();
        let over = |q: &Rect| q.x < r.x + r.w && r.x < q.x + q.w && q.y < r.y + r.h && r.y < q.y + q.h;
        let floated = d.core.nodes().iter().any(|n| n.text.as_deref() == Some(word) && over(&n.rect));
        assert!(!floated, "{word} floated over its field");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_narrow_window_folds_the_library_and_keeps_the_window_buttons() {
    let (store, dir) = temp_store("narrow");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 640.0, 600.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    d.frame(&mut app);
    assert!(d.key_of("library").is_none(), "the library folded away");
    let trash = d.key_of("t-trash").expect("the delete tool");
    let r = d.core.nodes().iter().find(|n| n.key == trash).map(|n| n.rect).unwrap();
    assert!(r.x + r.w <= 640.0, "inside the window: {r:?}");
    // Back: the list of notes takes the whole window, and the note goes.
    let back = d.key_of("t-back").expect("a way back to the list");
    d.click_key(&mut app, back);
    d.frame(&mut app);
    assert!(d.key_of("library").is_some(), "the list");
    assert!(d.key_of("t-trash").is_none(), "instead of the note");
    // Choosing a note returns to it.
    let row = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Welcome to Carnet"))
        .map(|n| n.rect.center())
        .expect("the welcome card");
    d.click(&mut app, row.x, row.y);
    d.frame(&mut app);
    assert!(d.key_of("library").is_none(), "the note again");
    assert!(d.key_of("t-back").is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_click_in_a_second_column_puts_the_caret_there() {
    let (mut app, mut d, dir) = body("cell-click");
    type_in(&mut app, &mut d, "/table");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "a");
    key(&mut app, &mut d, "tab");
    type_in(&mut app, &mut d, "bee");
    key(&mut app, &mut d, "down");
    d.frame(&mut app);
    let Kind::Table(t) = &blocks(&app)[0].kind else { panic!("{:?}", texts(&app)) };
    let target = Slot {
        block: blocks(&app)[0].id,
        cell: Some((t.rows[0].id, t.cols[1])),
    };
    let r = *app.cell_rects.get(&target).expect("the cell's box");
    // At the end of "bee", and in an empty cell's padding.
    d.click(&mut app, r.x + r.w - 6.0, r.y + r.h / 2.0);
    d.frame(&mut app);
    assert_eq!(app.caret.head, Pos { slot: target, byte: 3 });
    let empty = Slot {
        block: target.block,
        cell: Some((t.rows[1].id, t.cols[2])),
    };
    let r = *app.cell_rects.get(&empty).expect("an empty cell's box");
    d.click(&mut app, r.x + 3.0, r.y + 3.0);
    d.frame(&mut app);
    assert_eq!(app.caret.head.slot, empty);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_right_click_on_the_page_opens_its_menu() {
    let (mut app, mut d, dir) = body("page-menu");
    type_in(&mut app, &mut d, "hello world");
    key_with(&mut app, &mut d, "home", SHIFT);
    d.core.set_inspect(true);
    d.frame(&mut app);
    let k = *app.slot_keys.get(&app.caret.head.slot).unwrap();
    let at = d.core.caret_rect(k, 2).unwrap().center();
    d.input(&mut app, InputEvent::CursorMoved(at));
    d.input(
        &mut app,
        InputEvent::MouseDown {
            button: kui_native::MouseButton::Secondary,
            clicks: 1,
        },
    );
    d.input(
        &mut app,
        InputEvent::MouseUp {
            button: kui_native::MouseButton::Secondary,
        },
    );
    d.frame(&mut app);
    d.frame(&mut app);
    // Inside the selection: it stays, for the menu to act on.
    let (a, b) = page::ordered(app.doc(), app.caret);
    assert_eq!(b.byte - a.byte, 11);
    let bold = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Bold"))
        .map(|n| n.rect.center())
        .expect("the page's menu is open");
    d.click(&mut app, bold.x, bold.y);
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "**hello world**");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn one_selection_in_the_window() {
    let (mut app, mut d, dir) = body("one-selection");
    type_in(&mut app, &mut d, "body text");
    key_with(&mut app, &mut d, "home", SHIFT);
    assert!(!app.caret.is_empty());
    // The title takes the keyboard: the page's selection goes.
    let title = *app.fields.iter().find(|(_, f)| **f == Field::Title).unwrap().0;
    d.core.set_inspect(true);
    d.frame(&mut app);
    let r = d.core.nodes().iter().find(|n| n.key == title).map(|n| n.rect).expect("the title");
    d.click(&mut app, r.x + 10.0, r.y + r.h / 2.0);
    d.frame(&mut app);
    d.frame(&mut app);
    assert_eq!(d.core.focus(), Some(title));
    assert!(app.caret.is_empty(), "the body's selection let go");
    // And the other way: a selection in the title goes when the page
    // takes the keyboard back.
    d.text(&mut app, "Title");
    d.frame(&mut app);
    key_with(&mut app, &mut d, "home", SHIFT);
    let (a, b) = d.core.edit.selection_cursors(title).expect("a selection in the title");
    assert_ne!(a, b);
    key(&mut app, &mut d, "enter");
    d.frame(&mut app);
    assert!(page_focused(&app, &d));
    // kui keeps the title's range for when it is back, and draws none of
    // it meanwhile.
    let sel = d.core.edit.selection_cursors(title);
    assert!(sel.is_some_and(|(a, b)| a != b), "the title's range kept: {sel:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_empty_line_shows_its_caret() {
    let (mut app, mut d, dir) = body("empty-caret");
    d.core.set_inspect(true);
    d.frame(&mut app);
    let k = *app.slot_keys.get(&app.caret.head.slot).unwrap();
    let line = d.core.nodes().iter().find(|n| n.key == k).map(|n| n.rect).unwrap();
    let bar = d
        .core
        .nodes()
        .iter()
        .find(|n| n.rect.w == 2.0 && n.rect.h >= 8.0 && (n.rect.y - line.y).abs() < 4.0)
        .map(|n| n.rect);
    assert!(bar.is_some(), "a caret on the empty line at {line:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ctrl_b_wraps_the_selection_and_again_unwraps_it() {
    let (mut app, mut d, dir) = body("bold");
    type_in(&mut app, &mut d, "make this loud");
    for _ in 0..4 {
        key_with(&mut app, &mut d, "left", SHIFT);
    }
    key_with(&mut app, &mut d, "b", CTRL);
    assert_eq!(texts(&app)[0], "make this **loud**");
    key_with(&mut app, &mut d, "b", CTRL);
    assert_eq!(texts(&app)[0], "make this loud");
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(texts(&app)[0], "make this **loud**");
    let _ = std::fs::remove_dir_all(dir);
}

/// Thin solid lines drawn across the page's line at `slot`: what
/// underlines (or strikes through) its words.
fn lines_across(app: &mut Carnet, d: &mut Drive, slot: Slot) -> Vec<kui_native::Rect> {
    d.core.set_inspect(true);
    d.frame(app);
    let k = *app.slot_keys.get(&slot).unwrap();
    let row = d.core.nodes().iter().find(|n| n.key == k).map(|n| n.rect).unwrap();
    let (dl, _) = d.core.output();
    dl.quads
        .iter()
        .filter(|q| q.kind == kui_native::QuadKind::Solid && q.rect.h <= 3.0 && q.rect.w >= 8.0)
        .filter(|q| q.rect.y >= row.y && q.rect.y <= row.y + row.h && q.rect.x >= row.x - 1.0)
        .map(|q| q.rect)
        .collect()
}

/// ⌘U (Ctrl+U) underlines the selection: the file keeps `<u>…</u>`, the
/// page draws a line under the words, never the tags, and the toolbar's
/// Underline is on there; again, and it is all as it was.
#[test]
fn cmd_u_underlines_the_selection_in_the_file_and_on_the_page() {
    let (mut app, mut d, dir) = body("underline");
    type_in(&mut app, &mut d, "make this loud");
    let slot = app.caret.head.slot;
    for _ in 0..4 {
        key_with(&mut app, &mut d, "left", SHIFT);
    }
    let before = lines_across(&mut app, &mut d, slot).len();
    key_with(&mut app, &mut d, "u", CTRL);
    assert_eq!(texts(&app)[0], "make this <u>loud</u>");
    d.frame(&mut app);
    let tool = |d: &mut Drive| d.core.access_tree().nodes.iter().find(|n| n.name.as_deref() == Some("Underline")).and_then(|n| n.checked);
    assert_eq!(tool(&mut d), Some(true), "the toolbar's Underline is on");
    let lines = lines_across(&mut app, &mut d, slot);
    assert_eq!(lines.len(), before + 1, "a line under the word: {lines:?}");
    let word = (app.rect_at(&d.core, slot, "make this <u>".len()).unwrap(), app.rect_at(&d.core, slot, "make this <u>loud".len()).unwrap());
    let line = lines.iter().find(|r| (r.x - word.0.x).abs() < 2.0).copied().expect("under \"loud\"");
    assert!((line.x + line.w - word.1.x).abs() < 2.0, "as wide as the word: {line:?} {word:?}");
    assert!(line.y > word.0.y + word.0.h * 0.6, "under it, not through it: {line:?} {word:?}");
    assert!(page::style_at(app.doc(), app.caret).underline);
    app.save_current();
    let md = std::fs::read_to_string(app.store.notes[app.current].path.clone().unwrap()).unwrap();
    assert!(md.ends_with("make this <u>loud</u>\n"), "{md}");
    // Off again, the toolbar with it.
    key_with(&mut app, &mut d, "u", CTRL);
    assert_eq!(texts(&app)[0], "make this loud");
    d.frame(&mut app);
    assert_eq!(tool(&mut d), Some(false));
    assert_eq!(lines_across(&mut app, &mut d, slot).len(), before);
    // Nothing selected: on for what is typed, the toolbar's too.
    key(&mut app, &mut d, "end");
    key_with(&mut app, &mut d, "u", CTRL);
    d.frame(&mut app);
    assert_eq!(tool(&mut d), Some(true), "on for what is typed");
    type_in(&mut app, &mut d, " now");
    assert_eq!(texts(&app)[0], "make this loud <u>now</u>");
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: Bold showed on for a selection that only started in
/// bold. The toolbar's styles are on for a selection only when all of it
/// has them; Bold then makes it all bold, and shows on.
#[test]
fn a_style_shows_on_only_when_all_the_selection_has_it() {
    let (mut app, mut d, dir) = body("mixed-bold");
    type_in(&mut app, &mut d, "**bold** *it* plain");
    let slot = app.caret.head.slot;
    let tool = |d: &mut Drive, name: &str| d.core.access_tree().nodes.iter().find(|n| n.name.as_deref() == Some(name)).and_then(|n| n.checked);
    app.caret = Caret { anchor: Pos { slot, byte: 2 }, head: Pos { slot, byte: page::text(app.doc(), slot).len() } };
    d_frame_twice(&mut app, &mut d);
    assert_eq!(tool(&mut d, "Bold"), Some(false), "started in bold, not all bold");
    assert_eq!(tool(&mut d, "Italic"), Some(false));
    app.caret = Caret { anchor: Pos { slot, byte: 2 }, head: Pos { slot, byte: 6 } };
    d_frame_twice(&mut app, &mut d);
    assert_eq!(tool(&mut d, "Bold"), Some(true), "all bold");
    app.caret = Caret { anchor: Pos { slot, byte: 2 }, head: Pos { slot, byte: page::text(app.doc(), slot).len() } };
    d_frame_twice(&mut app, &mut d);
    key_with(&mut app, &mut d, "b", CTRL);
    d.frame(&mut app);
    assert_eq!(tool(&mut d, "Bold"), Some(true), "made all bold");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn folders_hold_notes_and_the_library_shows_one_at_a_time() {
    let (mut app, mut d, dir) = fresh("folders");
    d.text(&mut app, "Loose");
    d.frame(&mut app);
    app.save_current();
    // The header's button names a new folder; Enter makes it and shows it.
    let new = d.key_of("new-folder").expect("a new-folder button");
    d.click_key(&mut app, new);
    d.frame(&mut app);
    d.frame(&mut app);
    d.text(&mut app, "Work");
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    d.frame(&mut app);
    assert_eq!(app.store.folders, ["Work"]);
    assert_eq!(app.folder.as_deref(), Some("Work"));
    assert!(dir.join("Work").is_dir());
    assert!(app.order.is_empty(), "nothing in it yet");
    // A new note goes in the folder shown.
    app.new_note();
    d.frame(&mut app);
    d.text(&mut app, "Plan");
    d.frame(&mut app);
    app.save_current();
    assert!(dir.join("Work").join("Plan.md").exists());
    assert_eq!(app.order.len(), 1);
    // Moved in from the top level.
    let loose = app.store.notes.iter().position(|n| n.title == "Loose").unwrap();
    app.move_note(loose, "Work");
    assert!(dir.join("Work").join("Loose.md").exists());
    assert!(!dir.join("Loose.md").exists());
    assert_eq!(app.order.len(), 2);
    // Not empty: it stays.
    assert!(app.store.delete_folder("Work").is_err());
    // Reopened from disk, all as it was.
    let again = Store::open(dir.clone()).unwrap();
    assert_eq!(again.folders, ["Work"]);
    assert_eq!(again.notes.iter().filter(|n| n.folder == "Work").count(), 2);
    // All notes again.
    app.show_folder(None);
    assert_eq!(app.order.len(), 3);
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pinned_note_in_a_folder_stays_pinned() {
    let (store, dir) = temp_store("folder-pins");
    drop(store);
    std::fs::create_dir_all(dir.join("A")).unwrap();
    std::fs::write(dir.join("A").join("Same.md"), "# Same\n\nin a\n").unwrap();
    std::fs::write(dir.join("Same.md"), "# Same\n\ntop\n").unwrap();
    let mut s = Store::open(dir.clone()).unwrap();
    let i = s.notes.iter().position(|n| n.folder == "A").unwrap();
    s.notes[i].pinned = true;
    s.save_pins().unwrap();
    let s = Store::open(dir.clone()).unwrap();
    let pinned: Vec<&str> = s.notes.iter().filter(|n| n.pinned).map(|n| n.folder.as_str()).collect();
    assert_eq!(pinned, ["A"], "the one in the folder, not its namesake");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tab_still_walks_after_a_click_on_a_tool() {
    let (mut app, mut d, dir) = fresh("tab");
    d.core.set_focus(None);
    d.frame(&mut app);
    key(&mut app, &mut d, "tab");
    assert!(d.core.focus().is_some(), "Tab moved focus somewhere");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_file_is_renamed_once_the_title_lets_go() {
    let (mut app, mut d, dir) = fresh("rename");
    d.text(&mut app, "First");
    d.frame(&mut app);
    app.save_current();
    let first = app.store.notes[app.current].path.clone().unwrap();
    assert_eq!(first.file_name().unwrap(), "First.md");
    d.text(&mut app, " draft");
    d.frame(&mut app);
    app.save_current();
    assert!(first.exists(), "no rename letter by letter");
    key(&mut app, &mut d, "enter");
    d.frame(&mut app);
    assert!(dir.join("First draft.md").exists());
    assert!(!first.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_library_does_not_reshuffle_while_typing() {
    let (mut app, mut d, dir) = fresh("order");
    d.text(&mut app, "B");
    d.frame(&mut app);
    let order = app.order.clone();
    let older = order[1];
    app.open(older);
    d.frame(&mut app);
    app.edit_at(page::first(app.doc()));
    d.frame(&mut app);
    type_in(&mut app, &mut d, "!");
    assert_eq!(app.order, order);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_library_menu_deletes_the_note_it_was_opened_on() {
    let (mut app, mut d, dir) = fresh("menu");
    d.text(&mut app, "Doomed");
    d.frame(&mut app);
    app.save_current();
    let welcome = app.order.iter().copied().find(|&i| i != app.current).unwrap();
    app.open(welcome);
    d.frame(&mut app);
    d.core.set_inspect(true);
    d.frame(&mut app);
    let row = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Doomed"))
        .map(|n| n.rect)
        .expect("the row");
    d.input(&mut app, InputEvent::CursorMoved(row.center()));
    d.input(
        &mut app,
        InputEvent::MouseDown {
            button: kui_native::MouseButton::Secondary,
            clicks: 1,
        },
    );
    d.input(
        &mut app,
        InputEvent::MouseUp {
            button: kui_native::MouseButton::Secondary,
        },
    );
    d.frame(&mut app);
    d.frame(&mut app);
    let trash = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Move to Deleted"))
        .map(|n| n.rect.center())
        .expect("the menu is open");
    d.click(&mut app, trash.x, trash.y);
    d.frame(&mut app);
    assert_eq!(app.store.notes.len(), 1);
    assert_eq!(app.store.notes[app.current].title, "Welcome to Carnet");
    assert!(dir.join(".trash").join("Doomed.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// Not a pass/fail bound: prints what a frame costs on a big note, run with
/// `cargo test --release big_note -- --nocapture`.
#[test]
fn big_note_frame_cost() {
    let (mut app, mut d, dir) = body("perf");
    let mut list = Vec::new();
    for i in 0..1000 {
        list.push(Block::new(Kind::Item {
            style: ListStyle::Todo,
            indent: (i % 3) as u8,
            done: i % 4 == 0,
            num: 0,
            text: format!("item {i}: buy something reasonably long so it wraps sometimes"),
        }));
    }
    let cells = (0..200)
        .map(|r| (0..5).map(|c| format!("r{r} c{c}")).collect())
        .collect();
    list.push(Block::new(Kind::Table(Table::from_cells(cells))));
    list.push(Block::text(""));
    app.store.notes[app.current].doc = Doc::new(list);
    app.caret = Caret::at(page::first(app.doc()));
    let t0 = Instant::now();
    d.frame(&mut app);
    let first = t0.elapsed();
    let t0 = Instant::now();
    for _ in 0..20 {
        d.frame(&mut app);
    }
    let steady = t0.elapsed() / 20;
    let t0 = Instant::now();
    for _ in 0..20 {
        d.commit(&mut app, "x");
    }
    let typing = t0.elapsed() / 20;
    println!("2000 slots: first frame {first:?}, steady {steady:?}, keystroke+frame {typing:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A little picture file: 4×3 pixels of the accent.
fn png_file(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let rgba: Vec<u8> = (0..12).flat_map(|_| [194u8, 124, 104, 255]).collect();
    let png = crate::images::encode_png(4, 3, &rgba).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, png).unwrap();
    p
}

fn image_blocks(app: &Carnet) -> Vec<String> {
    app.doc()
        .blocks()
        .iter()
        .filter_map(|b| match &b.kind {
            Kind::Image { src, .. } => Some(src.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_pasted_picture_is_kept_beside_the_notes_and_shown() {
    let (mut app, mut d, dir) = body("paste-picture");
    type_in(&mut app, &mut d, "Look:");
    let rgba: Vec<u8> = (0..12).flat_map(|_| [10u8, 20, 30, 255]).collect();
    app.paste_picture(4, 3, &rgba);
    d.frame(&mut app);
    let srcs = image_blocks(&app);
    assert_eq!(srcs.len(), 1);
    assert!(srcs[0].starts_with("attachments/pasted-") && srcs[0].ends_with(".png"), "{srcs:?}");
    assert!(dir.join(&srcs[0]).is_file(), "the file is there");
    // The caret goes on below it, ready for more words.
    let tx = texts(&app);
    assert_eq!((tx.len(), tx[0].as_str(), tx[2].as_str()), (3, "Look:", ""));
    assert_eq!(app.caret.head.slot, Slot::block(blocks(&app)[2].id));
    // Drawn as a picture once decoded.
    d.core.set_inspect(true);
    d.frame(&mut app);
    d.frame(&mut app);
    // Its own shape, 4:3.
    let pic = d
        .core
        .nodes()
        .iter()
        .any(|n| n.role == Some(Role::Image) && (n.rect.w * 3.0 - n.rect.h * 4.0).abs() < 0.01);
    assert!(pic, "a picture node");
    // Written as Markdown, and the folder list does not list attachments.
    app.store.notes[app.current].title = "Shots".into();
    app.save_current();
    let md = std::fs::read_to_string(app.store.notes[app.current].path.clone().unwrap()).unwrap();
    assert!(md.contains(&format!("![]({})", srcs[0])), "{md}");
    app.rescan();
    assert!(app.store.folders.is_empty(), "{:?}", app.store.folders);
    // Undo takes it out of the note (the file stays, for redo).
    key_with(&mut app, &mut d, "z", CTRL);
    assert!(image_blocks(&app).is_empty());
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pictures_dropped_on_the_page_go_in_at_the_caret() {
    let (mut app, mut d, dir) = body("drop");
    let outside = std::env::temp_dir().join(format!("carnet-drop-src-{}", std::process::id()));
    let a = png_file(&outside, "Holiday photo.png");
    let txt = outside.join("notes.txt");
    std::fs::write(&txt, "hi").unwrap();
    let script = outside.join("run.sh");
    std::fs::write(&script, "echo hi").unwrap();
    d.frame(&mut app);
    let at = Vec2::new(600.0, 300.0);
    let paths = vec![a.display().to_string(), txt.display().to_string(), script.display().to_string()];
    d.input(&mut app, InputEvent::DragFiles { paths: paths.clone(), at });
    d.input(&mut app, InputEvent::DropFiles { paths, at });
    d.frame(&mut app);
    let srcs = image_blocks(&app);
    assert_eq!(srcs, ["attachments/Holiday-photo.png"]);
    assert!(dir.join("attachments").join("Holiday-photo.png").is_file());
    assert!(a.exists(), "copied, not moved");
    // Any other file goes in as a link to its copy, under the picture: a
    // document, and a script too (which asks before it opens).
    assert!(texts(&app).iter().any(|t| t == "[notes.txt](attachments/notes.txt) [run.sh](attachments/run.sh)"), "{:?}", texts(&app));
    assert!(dir.join("attachments").join("notes.txt").is_file() && dir.join("attachments").join("run.sh").is_file());
    assert!(app.notice.is_none(), "nothing left out");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

/// Markdown files dropped on the window come in as notes, never into the
/// note open: on the page, into the folder the library shows, the first
/// opened and chosen there; on a folder's row, into that folder, which
/// the library then shows. Dropped with a picture, the picture goes into
/// the note as ever, and that note stays open. One that can't come in
/// is said, by name.
#[test]
fn markdown_files_dropped_on_the_window_come_in_as_notes() {
    let (mut app, mut d, dir) = body("drop-md");
    let outside = std::env::temp_dir().join(format!("carnet-drop-md-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    let write = |name: &str, text: &[u8]| {
        let p = outside.join(name);
        std::fs::write(&p, text).unwrap();
        p.display().to_string()
    };
    let drop = |app: &mut Carnet, d: &mut Drive, paths: Vec<String>, at: Vec2| {
        d.input(app, InputEvent::DragFiles { paths: paths.clone(), at });
        d.frame(app);
        d.input(app, InputEvent::DropFiles { paths, at });
        d.frame(app);
    };
    let notice = |app: &Carnet| app.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    let title = |app: &Carnet| app.store.notes[app.current].title.clone();
    let page = Vec2::new(600.0, 300.0);
    let trip = write("Trip.md", b"# Trip\n\nTo the sea.\n");
    drop(&mut app, &mut d, vec![trip.clone(), write("Ideas.markdown", b"Some ideas\n")], page);
    assert_eq!(title(&app), "Trip", "the first opened");
    assert!(app.doc().contains("to the sea"));
    assert!(app.order.contains(&app.current), "and chosen in the library");
    assert!(dir.join("Trip.md").is_file() && dir.join("Ideas.md").is_file());
    assert!(outside.join("Trip.md").is_file(), "copied, not moved");
    assert_eq!(notice(&app), "Imported 2 notes");

    // On a folder's row: into it, and the library shows it.
    app.store.create_folder("", "Work").unwrap();
    app.store.create_folder("", "Home").unwrap();
    app.show_folder(Some("Home".into()));
    d_frame_twice(&mut app, &mut d);
    let work = app.folder_rects.iter().find(|(f, _)| f == "Work").map(|(_, r)| r.center()).expect("Work's row");
    drop(&mut app, &mut d, vec![trip], work);
    assert_eq!((title(&app), app.store.notes[app.current].folder.as_str()), ("Trip".to_string(), "Work"));
    assert_eq!(app.folder.as_deref(), Some("Work"));
    assert!(dir.join("Work/Trip.md").is_file());
    assert_eq!(notice(&app), "Imported \u{201c}Trip\u{201d} into \u{201c}Work\u{201d}");

    // With a picture, on the page: the picture into the note open, which
    // stays; the note into the folder shown.
    let pic = png_file(&outside, "Shell.png").display().to_string();
    drop(&mut app, &mut d, vec![pic, write("Later.md", b"# Later\n")], page);
    assert_eq!(title(&app), "Trip");
    assert_eq!(image_blocks(&app), ["../attachments/Shell.png"]);
    assert!(dir.join("Work/Later.md").is_file());
    assert_eq!(notice(&app), "Imported \u{201c}Later\u{201d} into \u{201c}Work\u{201d}");

    // Not text: left out, and said.
    drop(&mut app, &mut d, vec![write("Bad.md", &[0xff, 0xfe, b'x'])], page);
    assert_eq!(app.error.as_deref(), Some("Couldn't import \u{201c}Bad.md\u{201d}: it isn't text Carnet can read (not UTF-8)"));
    assert!(!dir.join("Work/Bad.md").exists());
    assert_eq!(title(&app), "Trip");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

/// The toolbar's paperclip asks for any file; what comes back goes in at
/// the caret: a picture as a picture, a document as a link among the
/// words, set apart from them by a space.
#[test]
fn the_paperclip_adds_pictures_and_documents_at_the_caret() {
    let (mut app, mut d, dir) = body("paperclip");
    type_in(&mut app, &mut d, "See the plan");
    for _ in 0..4 {
        key(&mut app, &mut d, "left");
    }
    d.frame(&mut app);
    // On the strip, or in More where the strip is too narrow for it.
    match d.key_of("t-attach") {
        Some(clip) => {
            d.click_key(&mut app, clip);
        }
        None => {
            assert!(app.overflow.iter().any(|t| matches!(t.msg, Msg::AddFiles)), "the paperclip, in More");
            app.on_plain(Msg::AddFiles);
        }
    }
    d.frame(&mut app);
    let asked = d.core.take_file_requests();
    assert_eq!(asked.len(), 1, "one dialog");
    assert!(asked[0].filters.is_empty(), "any file: {:?}", asked[0].filters);
    let outside = std::env::temp_dir().join(format!("carnet-clip-src-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    let pdf = outside.join("Trip plan (v2).pdf");
    std::fs::write(&pdf, b"%PDF-1.4").unwrap();
    app.insert_files(std::slice::from_ref(&pdf));
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "See the [Trip plan (v2).pdf](attachments/Trip-plan-%28v2%29.pdf) plan", "{:?}", texts(&app));
    assert_eq!(std::fs::read(dir.join("attachments/Trip-plan-(v2).pdf")).unwrap(), b"%PDF-1.4");
    // The link reads as one, and opens the copy.
    assert_eq!(drawn_text(&app, &d, app.caret.head.slot), "See the Trip plan (v2).pdf plan");
    let (_, _, url) = page::link_at(app.doc(), page::Caret::at(page::Pos { byte: 10, ..app.caret.head })).expect("a link");
    assert_eq!(app.store.linked_file("", &url).as_deref(), Some(dir.join("attachments/Trip-plan-(v2).pdf").as_path()));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

/// The finding: a `README` and a `.rs` file were refused, as nothing but
/// a PDF, a sheet or a recording was taken; then so was a program. Any
/// file goes in as a link now, one with no kind keeping none, a Markdown
/// file picked among them (dropped, it comes in as a note), a program, a
/// script and a shortcut as well; only a folder is left out.
#[test]
fn any_file_is_linked() {
    let (mut app, mut d, dir) = body("any-file");
    let outside = std::env::temp_dir().join(format!("carnet-any-file-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(outside.join("src")).unwrap();
    let names = ["README", "main.rs", "Plan.md", "setup.exe", "fix.py", "Home.lnk"];
    let mut picked: Vec<std::path::PathBuf> = names.iter().map(|n| outside.join(n)).collect();
    for f in &picked {
        std::fs::write(f, b"x").unwrap();
    }
    picked.push(outside.join("src"));
    app.insert_files(&picked);
    d.frame(&mut app);
    let links: Vec<String> = names.iter().map(|n| format!("[{n}](attachments/{n})")).collect();
    assert_eq!(texts(&app)[0], links.join(" "));
    for kept in names {
        assert!(dir.join("attachments").join(kept).is_file(), "{kept}");
    }
    assert!(app.store.notes.iter().all(|n| n.title != "Plan"), "a file, not a note");
    assert_eq!(app.notice.as_ref().map(|n| n.text.as_str()), Some("Left out a folder: only files go in"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

/// A link to a file opens it in its own app: a `README`, source code, a
/// PDF at once. One that could run (a program, a script, a shortcut) asks
/// first, naming it: Cancel (or Enter, which is on it, or Esc) opens
/// nothing, Open anyway opens it.
#[test]
fn a_link_to_a_file_that_could_run_asks_before_it_opens() {
    let (mut app, mut d, dir) = body("open-ask");
    d.core.set_inspect(true);
    let outside = std::env::temp_dir().join(format!("carnet-open-ask-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    let names = ["README", "main.rs", "setup.exe", "fix.py"];
    let picked: Vec<std::path::PathBuf> = names.iter().map(|n| outside.join(n)).collect();
    for f in &picked {
        std::fs::write(f, b"x").unwrap();
    }
    app.insert_files(&picked);
    d.frame(&mut app);
    let slot = app.caret.head.slot;
    let words = drawn_text(&app, &d, slot);
    assert_eq!(words, "README main.rs setup.exe fix.py");
    let opened = || store::OPENED.with_borrow_mut(std::mem::take).into_iter().map(std::path::PathBuf::from).collect::<Vec<_>>();
    let ctrl_click = |app: &mut Carnet, d: &mut Drive, name: &str| {
        let at = words.find(name).unwrap() + 1;
        d.input(app, InputEvent::Modifiers(CTRL));
        click_drawn(app, d, slot, at);
        d.input(app, InputEvent::Modifiers(KeyMods::NONE));
        d.frame(app);
    };
    let file = |n: &str| dir.join("attachments").join(n);
    opened();
    for n in ["README", "main.rs"] {
        ctrl_click(&mut app, &mut d, n);
        assert_eq!(opened(), [file(n)], "{n} opens at once");
        assert!(app.open_ask.is_none());
    }
    // A program: asked about, by name, and nothing opens yet.
    ctrl_click(&mut app, &mut d, "setup.exe");
    assert_eq!(opened(), Vec::<std::path::PathBuf>::new());
    assert!(app.open_ask.is_some());
    assert!(drawn(&mut d, &mut app, "Open \u{201c}setup.exe\u{201d}?") && drawn(&mut d, &mut app, super::open_view::OPEN_ASK_WHY));
    let cancel = d.key_named("Cancel").expect("Cancel");
    d.click_key(&mut app, cancel);
    d.frame(&mut app);
    assert!(app.open_ask.is_none() && opened().is_empty(), "Cancel opens nothing");
    // Enter is on Cancel; Esc is Cancel too.
    ctrl_click(&mut app, &mut d, "setup.exe");
    let before = texts(&app);
    key(&mut app, &mut d, "enter");
    assert!(app.open_ask.is_none() && opened().is_empty(), "Enter cancels");
    assert_eq!(texts(&app), before, "and types nothing");
    ctrl_click(&mut app, &mut d, "fix.py");
    assert!(app.open_ask.is_some(), "a script asks too");
    key(&mut app, &mut d, "escape");
    assert!(app.open_ask.is_none() && opened().is_empty(), "Esc cancels");
    // Open anyway: it opens.
    ctrl_click(&mut app, &mut d, "setup.exe");
    let anyway = d.key_named("Open anyway").expect("Open anyway");
    d.click_key(&mut app, anyway);
    d.frame(&mut app);
    assert_eq!(opened(), [file("setup.exe")]);
    assert!(app.open_ask.is_none());
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}


/// `/file` asks for files as the paperclip does, of any kind, and what
/// comes goes in on the line it was typed on, which it leaves empty.
#[test]
fn slash_file_asks_for_files_and_links_them_on_its_line() {
    let (mut app, mut d, dir) = body("slash-file");
    type_in(&mut app, &mut d, "Intro");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "/file");
    d.frame(&mut app);
    assert!(app.slash_now().is_some_and(|(_, shown)| crate::slash::COMMANDS[shown[0]].name == "File"), "offered first");
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app), ["Intro", ""], "the line left empty for the files");
    d.frame(&mut app);
    let asked = d.core.take_file_requests();
    assert_eq!(asked.len(), 1, "one dialog");
    assert!(asked[0].filters.is_empty(), "any file: {:?}", asked[0].filters);
    let outside = std::env::temp_dir().join(format!("carnet-slash-file-src-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    let pdf = outside.join("Tickets.pdf");
    std::fs::write(&pdf, b"%PDF-1.4").unwrap();
    app.insert_files(std::slice::from_ref(&pdf));
    d.frame(&mut app);
    assert_eq!(texts(&app), ["Intro", "[Tickets.pdf](attachments/Tickets.pdf)"]);
    assert!(dir.join("attachments/Tickets.pdf").is_file());
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

/// Shift+F3 goes round the cases of the word the caret is in, as Word's
/// does, the caret staying; the page's menu puts the selection in one,
/// Markdown's marks as they were. Each is one undo.
#[test]
fn the_case_changes_by_shift_f3_and_the_menu() {
    let (mut app, mut d, dir) = body("case");
    type_in(&mut app, &mut d, "hello **brave** world");
    let at = app.caret;
    for want in ["hello **brave** WORLD", "hello **brave** World", "hello **brave** world"] {
        key_with(&mut app, &mut d, "f3", SHIFT);
        assert_eq!(texts(&app)[0], want);
    }
    assert_eq!(app.caret, at, "the caret where it was");
    key_with(&mut app, &mut d, "a", CTRL);
    app.page_action("case:title");
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "Hello **Brave** World");
    app.page_action("case:upper");
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "HELLO **BRAVE** WORLD");
    app.page_action("case:sentence");
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "Hello **brave** world");
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(texts(&app)[0], "HELLO **BRAVE** WORLD", "one undo");
    // In the page's menu, under Change case.
    let menu = app.page_menu();
    let sub = menu.iter().find(|i| i.label == "Change case").expect("Change case");
    let names: Vec<&str> = sub.submenu.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(names, ["lowercase", "UPPERCASE", "Capitalize Each Word", "Sentence case"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// Dropped under the words, past the empty line after them: in that
/// line's place, as a paste is, so the file has one blank line between
/// the words and the picture.
#[test]
fn a_picture_dropped_under_the_words_leaves_no_gap() {
    let (mut app, mut d, dir) = body("drop-gap");
    type_in(&mut app, &mut d, "A dropped photo:");
    key(&mut app, &mut d, "enter");
    d.frame(&mut app);
    let outside = std::env::temp_dir().join(format!("carnet-drop-gap-{}", std::process::id()));
    let pic = png_file(&outside, "leaf-big.png");
    let last = app.row_rects.last().unwrap().1;
    let at = Vec2::new(last.x + 40.0, last.y + last.h + 60.0);
    let paths = vec![pic.display().to_string()];
    d.input(&mut app, InputEvent::DragFiles { paths: paths.clone(), at });
    d.frame(&mut app);
    d.frame(&mut app);
    assert_eq!(app.drop_index, Some(2), "past the empty line");
    d.input(&mut app, InputEvent::DropFiles { paths, at });
    d.frame(&mut app);
    app.save_current();
    let md = std::fs::read_to_string(app.store.notes[app.current].path.clone().unwrap()).unwrap();
    assert!(md.contains("A dropped photo:\n\n![](attachments/leaf-big.png)"), "{md:?}");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn a_note_moved_to_a_folder_keeps_its_pictures() {
    let (mut app, mut d, dir) = body("relink");
    let rgba: Vec<u8> = (0..12).flat_map(|_| [10u8, 20, 30, 255]).collect();
    app.paste_picture(4, 3, &rgba);
    app.store.notes[app.current].title = "Trip".into();
    app.save_current();
    let work = app.store.create_folder("", "Work").unwrap();
    let i = app.current;
    app.move_note(i, &work);
    d.frame(&mut app);
    let src = image_blocks(&app).remove(0);
    assert!(src.starts_with("../attachments/pasted-"), "{src}");
    let resolved = app.store.resolve("Work", &src).unwrap();
    assert!(resolved.is_file(), "{resolved:?}");
    let md = std::fs::read_to_string(dir.join("Work").join("Trip.md")).unwrap();
    assert!(md.contains(&src), "{md}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_copy_carries_html_beside_the_markdown() {
    let (mut app, mut d, dir) = body("copy-html");
    type_in(&mut app, &mut d, "Some **bold** words");
    key_with(&mut app, &mut d, "home", SHIFT);
    key_with(&mut app, &mut d, "c", CTRL);
    let html = d.core.take_menu_actions().into_iter().find_map(|a| match a {
        kui_native::MenuAction::SetClipboard { text, html } => Some((text, html)),
        _ => None,
    });
    let (text, html) = html.expect("a copy");
    assert_eq!(text, "Some **bold** words");
    assert_eq!(html.as_deref(), Some("<p>Some <b>bold</b> words</p>"));
    let _ = std::fs::remove_dir_all(dir);
}

/// Lines copied from a code block: the text is the lines, the HTML is
/// code (`<pre><code>`, its language named), not paragraphs and `<br>`s.
#[test]
fn code_copied_carries_code_as_its_html() {
    let (mut app, mut d, dir) = note_of("copy-code-html", vec![Kind::Raw("```rust\nlet a = 1;\nlet b = 2;\n```".into()), Kind::Text(String::new())]);
    let slot = Slot::block(blocks(&app)[0].id);
    let (lo, hi) = page::code_body(&raw(&app, 0)).unwrap();
    app.caret = Caret { anchor: Pos { slot, byte: lo }, head: Pos { slot, byte: hi } };
    d.frame(&mut app);
    key_with(&mut app, &mut d, "c", CTRL);
    let (text, html) = d
        .core
        .take_menu_actions()
        .into_iter()
        .find_map(|a| match a {
            kui_native::MenuAction::SetClipboard { text, html } => Some((text, html)),
            _ => None,
        })
        .expect("a copy");
    assert_eq!(text, "let a = 1;\nlet b = 2;");
    assert_eq!(html.as_deref(), Some("<pre><code class=\"language-rust\">let a = 1;\nlet b = 2;</code></pre>"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn settings_open_change_the_page_and_close() {
    let (mut app, mut d, dir) = body("settings");
    type_in(&mut app, &mut d, "hello");
    d.core.set_inspect(true);
    d.frame(&mut app);
    let k = *app.slot_keys.get(&app.caret.head.slot).unwrap();
    let before = d.core.caret_rect(k, 0).unwrap().h;
    // Ctrl+, from the page opens them in its place.
    key_with(&mut app, &mut d, ",", CTRL);
    d.frame(&mut app);
    assert!(app.settings_open);
    assert!(d.key_of("t-trash").is_none(), "the note's tools are gone");
    for _ in 0..4 {
        let up = d.key_of("size-up").expect("a bigger-text button");
        d.click_key(&mut app, up);
        d.frame(&mut app);
    }
    assert_eq!(app.settings.size, 20);
    // Dark, whatever the OS says.
    let dark = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Dark"))
        .map(|n| n.rect.center())
        .expect("the dark choice");
    d.click(&mut app, dark.x, dark.y);
    d.frame(&mut app);
    assert_eq!(app.settings.theme, ThemePref::Dark);
    assert_eq!(d.core.theme().appearance, kui_native::Appearance::Dark);
    // The font list opens with the default first: its pill scrolled up
    // into the window, past the letters' rows.
    let pill = d.key_of("font-body").expect("the text font");
    for _ in 0..20 {
        if d.rect_of(pill).is_some_and(|r| r.y < 400.0) {
            break;
        }
        d.wheel(&mut app, 700.0, 400.0, 0.0, -120.0);
        d.frame(&mut app);
    }
    assert!(d.rect_of(pill).is_some_and(|r| r.y < 400.0), "{:?}", d.rect_of(pill));
    d.click_key(&mut app, pill);
    d.frame(&mut app);
    assert!(d.core.nodes().iter().any(|n| n.text.as_deref() == Some("System")));
    assert!(app.fields.values().any(|f| *f == Field::Font), "with its search");
    // Back: the note, its text bigger.
    let back = d.key_of("t-close-settings").unwrap();
    d.click_key(&mut app, back);
    d.frame(&mut app);
    d.frame(&mut app);
    assert!(!app.settings_open);
    let k = *app.slot_keys.get(&app.caret.head.slot).unwrap();
    let after = d.core.caret_rect(k, 0).unwrap().h;
    assert!(after > before + 2.0, "{before} -> {after}");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_notes_folder_can_move() {
    let (mut app, mut d, dir) = fresh("move-dir");
    d.text(&mut app, "Stays here");
    d.frame(&mut app);
    let other = std::env::temp_dir().join(format!("carnet-ui-other-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&other);
    app.settings_open = true;
    app.dir_change = Some(Some(other.clone()));
    d.frame(&mut app);
    d.frame(&mut app);
    assert_eq!(app.store.dir, other);
    assert_eq!(app.settings.dir.as_deref(), Some(other.as_path()));
    assert!(app.settings_open, "still in the settings");
    assert!(dir.join("Stays here.md").exists(), "saved before the move");
    assert!(other.join("Welcome to Carnet.md").exists(), "a new folder starts with the welcome");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(other);
}

/// A right-click at byte 2 of the caret's line.
fn right_click_page(app: &mut Carnet, d: &mut Drive) {
    d.core.set_inspect(true);
    d.frame(app);
    let k = *app.slot_keys.get(&app.caret.head.slot).unwrap();
    let at = d.core.caret_rect(k, 2).unwrap().center();
    d.input(app, InputEvent::CursorMoved(at));
    d.input(app, InputEvent::MouseDown { button: kui_native::MouseButton::Secondary, clicks: 1 });
    d.input(app, InputEvent::MouseUp { button: kui_native::MouseButton::Secondary });
    d.frame(app);
    d.frame(app);
}

#[test]
fn a_click_anywhere_outside_a_menu_closes_it() {
    let (mut app, mut d, dir) = body("menu-outside");
    type_in(&mut app, &mut d, "hello world");
    // On the page.
    right_click_page(&mut app, &mut d);
    assert!(d.core.menu().is_some());
    d.click(&mut app, 900.0, 600.0);
    d.frame(&mut app);
    assert!(d.core.menu().is_none(), "a click on the page");
    // On the toolbar's empty strip, which drags the window otherwise.
    right_click_page(&mut app, &mut d);
    assert!(d.core.menu().is_some());
    d.click(&mut app, 960.0, 25.0);
    d.frame(&mut app);
    assert!(d.core.menu().is_none(), "a click on the titlebar");
    d.frame(&mut app);
    // And the window going to the background.
    right_click_page(&mut app, &mut d);
    d.core.set_focused(false);
    d.frame(&mut app);
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(500.0, 500.0)));
    d.frame(&mut app);
    assert!(d.core.menu().is_none(), "another app");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn choosing_a_note_closes_the_settings_even_the_open_one() {
    let (mut app, mut d, dir) = body("settings-close");
    key_with(&mut app, &mut d, ",", CTRL);
    assert!(app.settings_open);
    d.core.set_inspect(true);
    d.frame(&mut app);
    let card = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Welcome to Carnet"))
        .map(|n| n.rect.center())
        .expect("a card");
    d.click(&mut app, card.x, card.y);
    d.frame(&mut app);
    assert!(!app.settings_open);
    // The open note's own card too.
    key_with(&mut app, &mut d, ",", CTRL);
    d.frame(&mut app);
    assert!(app.settings_open);
    let current = app.current as u64;
    app.on_plain(Msg::OpenNote { note: current });
    assert!(!app.settings_open);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn alt_tab_leaves_the_page_its_keyboard() {
    let (mut app, mut d, dir) = body("alt-tab");
    type_in(&mut app, &mut d, "words");
    let alt = KeyMods { shift: false, ctrl: false, alt: true, super_key: false };
    key_with(&mut app, &mut d, "tab", alt);
    assert!(page_focused(&app, &d), "Alt+Tab is the OS's");
    assert_eq!(texts(&app)[0], "words", "and types nothing");
    // The window loses the keyboard and gets it back: so does the page.
    d.core.set_focused(false);
    d.frame(&mut app);
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(500.0, 500.0)));
    d.core.set_focus(None);
    d.core.set_focused(true);
    d.frame(&mut app);
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(501.0, 500.0)));
    d.frame(&mut app);
    d.frame(&mut app);
    assert!(page_focused(&app, &d), "back with the window");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_slash_opens_the_blocks_menu() {
    let (mut app, mut d, dir) = body("slash");
    d.core.set_inspect(true);
    // An empty line says what it can do.
    d.frame(&mut app);
    let hinted = d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.contains("/ for blocks")));
    assert!(hinted, "a hint on the empty line");
    type_in(&mut app, &mut d, "/");
    d.frame(&mut app);
    let shown = |d: &Drive, name: &str| d.core.nodes().iter().any(|n| n.text.as_deref() == Some(name));
    assert!(shown(&d, "Heading 1") && shown(&d, "Table") && shown(&d, "Picture"));
    // Narrowed as it is typed; Down and Enter choose.
    type_in(&mut app, &mut d, "head");
    d.frame(&mut app);
    assert!(!shown(&d, "Table"));
    key(&mut app, &mut d, "down");
    key(&mut app, &mut d, "enter");
    assert!(matches!(&blocks(&app)[0].kind, Kind::Heading { level: 2, text } if text.is_empty()));
    type_in(&mut app, &mut d, "Plan");
    key(&mut app, &mut d, "enter");
    // A click on a row: a divider.
    type_in(&mut app, &mut d, "/div");
    d.frame(&mut app);
    let row = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Divider"))
        .map(|n| n.rect.center())
        .expect("the divider row");
    d.click(&mut app, row.x, row.y);
    d.frame(&mut app);
    assert!(texts(&app).contains(&"---".to_string()), "{:?}", texts(&app));
    let _ = std::fs::remove_dir_all(dir);
}

/// In a quote the `/` menu shows and Tab takes its choice, as in a
/// paragraph: not a tab typed after the word.
#[test]
fn the_slash_menu_works_in_a_quote() {
    let (mut app, mut d, dir) = body("slash-quote");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "> ");
    assert!(matches!(blocks(&app)[0].kind, Kind::Quote(_)));
    for c in ["/", "c", "o", "d", "e"] {
        key(&mut app, &mut d, c);
    }
    d.frame(&mut app);
    assert!(app.slash_now().is_some());
    assert!(d.core.nodes().iter().any(|n| n.text.as_deref() == Some("Code block")), "the menu shows");
    key(&mut app, &mut d, "tab");
    assert!(matches!(&blocks(&app)[0].kind, Kind::Raw(r) if r.starts_with("```")), "{:?}", texts(&app));
    assert!(!texts(&app).iter().any(|t| t.contains('\t') || t.contains("/code")), "{:?}", texts(&app));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn esc_closes_the_slash_menu_and_keeps_the_words() {
    let (mut app, mut d, dir) = body("slash-esc");
    type_in(&mut app, &mut d, "/ta");
    d.frame(&mut app);
    assert!(app.slash_now().is_some());
    key(&mut app, &mut d, "escape");
    assert!(app.slash_now().is_none());
    assert_eq!(texts(&app)[0], "/ta");
    // Enter is Enter again.
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[..2], ["/ta", ""]);
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: `/code`, Enter, a line, ↓ and "after code" made the
/// closing fence `` ```after code ``. ↓ from the code's last line leaves
/// it for the line below, and ↑ from there comes back to that line, not
/// to the fence.
#[test]
fn down_from_codes_last_line_leaves_it() {
    let (mut app, mut d, dir) = body("code-down");
    type_in(&mut app, &mut d, "/code");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "let x = 1;");
    key(&mut app, &mut d, "down");
    type_in(&mut app, &mut d, "after code");
    assert!(matches!(&blocks(&app)[0].kind, Kind::Raw(r) if r == "```\nlet x = 1;\n```"), "{:?}", texts(&app));
    assert_eq!(texts(&app)[1], "after code");
    key(&mut app, &mut d, "up");
    type_in(&mut app, &mut d, "!");
    // On the code's line, under the caret's column; the fences whole.
    let Kind::Raw(r) = &blocks(&app)[0].kind else { panic!("{:?}", texts(&app)) };
    let lines: Vec<&str> = r.lines().collect();
    assert!(lines.len() == 3 && lines[0] == "```" && lines[1].contains('!') && lines[2] == "```", "{r:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A note of `kinds`, the caret at the end of the first block, its lines
/// laid out with their text.
fn note_of(name: &str, kinds: Vec<Kind>) -> (Carnet, Drive, std::path::PathBuf) {
    let (mut app, mut d, dir) = body(name);
    app.edit(Step::Other, |doc, c| {
        *doc = carnet_core::doc::Doc::new(kinds.into_iter().map(Block::new).collect());
        *c = Caret::at(page::end(doc, Slot::block(doc.blocks()[0].id)));
        true
    });
    d.core.set_inspect(true);
    for _ in 0..3 {
        d.frame(&mut app);
    }
    (app, d, dir)
}

/// A press on the drawn words of `slot` at place `at`, counted as the
/// `clicks`th (two a double click, three a triple).
fn press_drawn(app: &mut Carnet, d: &mut Drive, slot: Slot, at: usize, clicks: u8) {
    let k = *app.slot_keys.get(&slot).unwrap();
    let r = d.core.caret_rect(k, at).unwrap();
    d.input(app, InputEvent::CursorMoved(Vec2::new(r.x + 1.0, r.y + r.h / 2.0)));
    d.input(app, InputEvent::MouseDown { button: kui_native::MouseButton::Primary, clicks });
    d.input(app, InputEvent::MouseUp { button: kui_native::MouseButton::Primary });
    d.frame(app);
    d.frame(app);
}

/// A drag over the drawn words, from place `from` of one slot to place
/// `to` of another.
fn drag_drawn(app: &mut Carnet, d: &mut Drive, (s, from): (Slot, usize), (t, to): (Slot, usize)) {
    let a = d.core.caret_rect(*app.slot_keys.get(&s).unwrap(), from).unwrap();
    let b = d.core.caret_rect(*app.slot_keys.get(&t).unwrap(), to).unwrap();
    d.drag(app, Vec2::new(a.x + 1.0, a.y + a.h / 2.0), Vec2::new(b.x + 1.0, b.y + b.h / 2.0));
    d.frame(app);
}

fn raw(app: &Carnet, i: usize) -> String {
    match &blocks(app)[i].kind {
        Kind::Raw(t) => t.clone(),
        k => panic!("no code: {k:?}"),
    }
}

/// The finding: the desktop drew a code block as written, its fences and
/// language and all, where the web shows only its lines: Markdown while
/// writing. Only the lines between the fences are drawn, and a click, a
/// double or triple click, a drag, Ctrl+A, a copy and typing over what is
/// chosen all read them as the code's own bytes; the fences stay.
#[test]
fn a_code_block_is_drawn_without_its_fences() {
    let code = "```rust\nlet x = 1;\nlet y = 2;\n```";
    let lines = "let x = 1;\nlet y = 2;";
    let (mut app, mut d, dir) = note_of("code-drawn", vec![Kind::Text("Above".into()), Kind::Raw(code.into()), Kind::Text("Below".into())]);
    let slot = Slot::block(blocks(&app)[1].id);
    let (above, below) = (Slot::block(blocks(&app)[0].id), Slot::block(blocks(&app)[2].id));
    let drawn = drawn_text(&app, &d, slot);
    assert_eq!(drawn, lines, "neither fence nor language drawn");
    // A click at the first letter: the code's first byte; further in,
    // the byte under it, and the caret drawn there.
    press_drawn(&mut app, &mut d, slot, 0, 1);
    assert_eq!(app.caret, Caret::at(Pos { slot, byte: 8 }));
    press_drawn(&mut app, &mut d, slot, 15, 1);
    assert_eq!(app.caret, Caret::at(Pos { slot, byte: 8 + 15 }));
    let k = *app.slot_keys.get(&slot).unwrap();
    assert_eq!(app.rect_at(&d.core, slot, 8 + 15), d.core.caret_rect(k, 15));
    assert_eq!(app.rect_at(&d.core, slot, 0), d.core.caret_rect(k, 0), "a fence at the lines' nearer end");
    type_in(&mut app, &mut d, "!");
    assert_eq!(raw(&app, 1), "```rust\nlet x = 1;\nlet !y = 2;\n```");
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(raw(&app, 1), code);
    // Home and End: the drawn line's ends, never a fence.
    press_drawn(&mut app, &mut d, slot, 2, 1);
    key(&mut app, &mut d, "home");
    assert_eq!(app.caret.head.byte, 8);
    key(&mut app, &mut d, "end");
    assert_eq!(app.caret.head.byte, 8 + "let x = 1;".len());
    press_drawn(&mut app, &mut d, slot, 15, 1);
    key(&mut app, &mut d, "end");
    assert_eq!(app.caret.head.byte, 8 + lines.len());
    // A double click: the word under it.
    press_drawn(&mut app, &mut d, slot, 1, 1);
    press_drawn(&mut app, &mut d, slot, 1, 2);
    assert_eq!(page::copy(app.doc(), app.caret).as_deref(), Some("let"));
    // A triple click, Ctrl+A and a drag end to end: the lines, no fence.
    let whole = Caret { anchor: Pos { slot, byte: 8 }, head: Pos { slot, byte: 8 + lines.len() } };
    press_drawn(&mut app, &mut d, slot, 4, 1);
    press_drawn(&mut app, &mut d, slot, 4, 2);
    press_drawn(&mut app, &mut d, slot, 4, 3);
    assert_eq!(app.caret, whole, "a triple click");
    press_drawn(&mut app, &mut d, slot, 4, 1);
    key_with(&mut app, &mut d, "a", CTRL);
    assert_eq!(app.caret, whole, "Ctrl+A");
    press_drawn(&mut app, &mut d, slot, 0, 1);
    drag_drawn(&mut app, &mut d, (slot, 0), (slot, lines.len()));
    assert_eq!(app.caret, whole, "a drag");
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some(lines), "the lines alone");
    // Typed over, the lines go and the fences stay.
    type_in(&mut app, &mut d, "fn main() {}");
    assert_eq!(raw(&app, 1), "```rust\nfn main() {}\n```");
    assert_eq!(drawn_text(&app, &d, slot), "fn main() {}");
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(raw(&app, 1), code);
    // A drag from the code's first letter on past it copies it as code,
    // fences and all; from part-way in, its fences round what it took.
    drag_drawn(&mut app, &mut d, (slot, 0), (below, 2));
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some("```rust\nlet x = 1;\nlet y = 2;\n```\n\nBe\n"));
    drag_drawn(&mut app, &mut d, (above, 2), (slot, 3));
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some("ove\n\n```rust\nlet\n```\n"));
    // Taken out part-way, code keeps its fences: what is left after the
    // selection joins its lines, below; code below stays code.
    drag_drawn(&mut app, &mut d, (slot, 15), (below, 2));
    key(&mut app, &mut d, "backspace");
    assert_eq!(raw(&app, 1), "```rust\nlet x = 1;\nlet low\n```");
    assert_eq!(app.caret, Caret::at(Pos { slot, byte: 8 + 15 }));
    key_with(&mut app, &mut d, "z", CTRL);
    drag_drawn(&mut app, &mut d, (above, 2), (slot, 4));
    key(&mut app, &mut d, "backspace");
    assert_eq!(texts(&app)[..2], ["Ab".to_string(), "```rust\nx = 1;\nlet y = 2;\n```".to_string()]);
    let _ = std::fs::remove_dir_all(dir);
}

/// Code with nothing between its fences is drawn as one empty line, no
/// fence; a click there puts the caret on it, and what is typed goes
/// between the fences.
#[test]
fn an_empty_code_block_is_one_empty_line_to_write_in() {
    for fence in ["```", "```rust"] {
        let (mut app, mut d, dir) = note_of("code-empty", vec![Kind::Text("Above".into()), Kind::Raw(format!("{fence}\n```")), Kind::Text("Below".into())]);
        let slot = Slot::block(blocks(&app)[1].id);
        let drawn = drawn_text(&app, &d, slot);
        assert!(drawn.trim_matches('\u{200B}').is_empty(), "{drawn:?}");
        // A click on the line (nothing on it to measure a place by).
        let k = *app.slot_keys.get(&slot).unwrap();
        let line = d.core.nodes().iter().find(|n| n.key == k).map(|n| n.rect).unwrap();
        d.click(&mut app, line.x + 2.0, line.y + line.h / 2.0);
        d.frame(&mut app);
        assert_eq!(app.caret, Caret::at(Pos { slot, byte: fence.len() + 1 }));
        type_in(&mut app, &mut d, "let x;");
        assert_eq!(raw(&app, 1), format!("{fence}\nlet x;\n```"));
        assert_eq!(drawn_text(&app, &d, slot), "let x;");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Front matter is drawn as its lines, muted, without its `---`.
#[test]
fn front_matter_is_drawn_without_its_dashes() {
    let (mut app, mut d, dir) = note_of("front-drawn", vec![Kind::Raw("---\ntitle: Hi\ntags: [a]\n---".into()), Kind::Text("Words".into())]);
    let slot = Slot::block(blocks(&app)[0].id);
    assert_eq!(drawn_text(&app, &d, slot), "title: Hi\ntags: [a]");
    press_drawn(&mut app, &mut d, slot, 0, 1);
    assert_eq!(app.caret, Caret::at(Pos { slot, byte: 4 }));
    type_in(&mut app, &mut d, "x");
    assert_eq!(raw(&app, 0), "---\nxtitle: Hi\ntags: [a]\n---");
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: Enter at the end of "> quoted words", then `/`: no menu,
/// and `/h2` typed as words. Any line of a quote offers it.
#[test]
fn the_slash_menu_works_on_a_quotes_next_line() {
    let (mut app, mut d, dir) = body("slash-quote-line");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "> ");
    type_in(&mut app, &mut d, "quoted words");
    key(&mut app, &mut d, "enter");
    for c in ["/", "h", "2"] {
        key(&mut app, &mut d, c);
    }
    d.frame(&mut app);
    assert!(app.slash_now().is_some(), "{:?}", texts(&app));
    assert!(d.core.nodes().iter().any(|n| n.text.as_deref() == Some("Heading 2")), "the menu shows");
    key(&mut app, &mut d, "enter");
    assert!(matches!(&blocks(&app)[0].kind, Kind::Quote(t) if t == "quoted words"), "{:?}", texts(&app));
    assert!(matches!(&blocks(&app)[1].kind, Kind::Heading { level: 2, text } if text.is_empty()), "{:?}", texts(&app));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_shipped_faces_are_registered_and_used() {
    let (mut app, mut d, dir) = body("fonts");
    d.frame(&mut app);
    let families = d.core.system_font_families();
    assert!(families.iter().any(|f| f == "Inter"), "Inter");
    assert!(families.iter().any(|f| f == "Source Serif 4"), "Source Serif 4");
    let _ = std::fs::remove_dir_all(dir);
}

/// A note with a 400×300 picture pasted into it, selected.
fn with_picture(name: &str) -> (Carnet, Drive, std::path::PathBuf) {
    let (mut app, mut d, dir) = body(name);
    type_in(&mut app, &mut d, "Above");
    let rgba: Vec<u8> = (0..400 * 300).flat_map(|_| [200u8, 150, 120, 255]).collect();
    app.paste_picture(400, 300, &rgba);
    d.frame(&mut app);
    d.frame(&mut app);
    let i = texts(&app).len() - 2;
    let c = page::select_blocks(app.doc(), i, i);
    app.caret = c;
    d.core.set_inspect(true);
    d.frame(&mut app);
    d.frame(&mut app);
    (app, d, dir)
}

/// Ctrl+C on a chosen picture puts its pixels on the clipboard, through
/// kui as text goes.
#[test]
fn a_chosen_picture_copies_as_a_picture() {
    let (mut app, mut d, dir) = with_picture("copy-picture");
    d.core.take_menu_actions();
    key_with(&mut app, &mut d, "c", CTRL);
    let picture = d.core.take_menu_actions().into_iter().find_map(|a| match a {
        kui_native::MenuAction::SetClipboardImage { width, height, rgba } => Some((width, height, rgba)),
        _ => None,
    });
    let (w, h, rgba) = picture.expect("a picture on the clipboard");
    assert_eq!((w, h, rgba.len()), (400, 300, 400 * 300 * 4));
    assert_eq!(&rgba[..4], &[200, 150, 120, 255]);
    let _ = std::fs::remove_dir_all(dir);
}

fn image_width(app: &Carnet) -> Option<Option<u32>> {
    app.doc().blocks().iter().find_map(|b| match &b.kind {
        Kind::Image { width, .. } => Some(*width),
        _ => None,
    })
}

#[test]
fn a_picture_resizes_from_its_corner_and_keeps_its_shape() {
    let (mut app, mut d, dir) = with_picture("resize");
    let handle = d.key_of("resize").expect("a handle on the selected picture");
    let r = d.core.nodes().iter().find(|n| n.key == handle).map(|n| n.rect).unwrap();
    let from = r.center();
    d.drag(&mut app, from, Vec2::new(from.x - 150.0, from.y + 3.0));
    d.frame(&mut app);
    // A little past the drag's slop: about 150 narrower.
    let w = image_width(&app).flatten().expect("a width");
    assert!((240..=260).contains(&w), "{w}");
    // Drawn at that width, the height following (3:4).
    let pic = d
        .core
        .nodes()
        .iter()
        .filter(|n| n.role == Some(Role::Image))
        .map(|n| n.rect)
        .min_by(|a, b| a.w.total_cmp(&b.w))
        .unwrap();
    assert!((pic.w - w as f32).abs() < 1.0 && (pic.h - pic.w * 0.75).abs() < 1.0, "{pic:?}");
    // Kept in the Markdown, Obsidian's way.
    app.store.notes[app.current].title = "Sized".into();
    app.save_current();
    let md = std::fs::read_to_string(app.store.notes[app.current].path.clone().unwrap()).unwrap();
    assert!(md.contains(&format!("|{w}](attachments/")), "{md}");
    // One undo takes it back to its own size.
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(image_width(&app), Some(None));
    let _ = std::fs::remove_dir_all(dir);
}

/// Pictures side by side at an edge: one resized by its handle takes the
/// height of the one beside it when it comes within a few pixels, so the
/// row lines up — a thin line under both says so while dragged — and its
/// width follows its shape. Further off, it's as dragged.
#[test]
fn a_picture_resized_in_a_row_snaps_to_its_neighbours_height() {
    let (mut app, mut d, dir) = library("pic-snap", &[("Row", "![a|left|200](attachments/a.png)\n\n![b|left|160](attachments/b.png)\n\nWords beside them")]);
    png_file(&dir.join("attachments"), "a.png");
    let square: Vec<u8> = (0..16).flat_map(|_| [94u8, 124, 164, 255]).collect();
    std::fs::write(dir.join("attachments").join("b.png"), crate::images::encode_png(4, 4, &square).unwrap()).unwrap();
    app.store.index_files();
    app.images.retry_failed();
    app.open(app.store.notes.iter().position(|n| n.title == "Row").unwrap());
    d_frame_twice(&mut app, &mut d);
    let width_of = |app: &Carnet, alt: &str| {
        app.doc().blocks().iter().find_map(|b| match &b.kind {
            Kind::Image { alt: a, width, .. } if a == alt => Some(*width),
            _ => None,
        })
    };
    let pick_b = |app: &mut Carnet, d: &mut Drive| {
        let i = app.doc().blocks().iter().position(|b| matches!(&b.kind, Kind::Image { alt, .. } if alt == "b")).unwrap();
        app.caret = page::select_blocks(app.doc(), i, i);
        app.editing = true;
        d_frame_twice(app, d);
    };
    // a is 200 × 150; b, square, 160 × 160. b dragged 7 narrower, 153
    // tall, is 3 off a's height: it takes it, 150 wide.
    pick_b(&mut app, &mut d);
    let handle = d.key_of("resize-r").expect("b's handle");
    let from = d.core.nodes().iter().find(|n| n.key == handle).map(|n| n.rect.center()).unwrap();
    d.input(&mut app, InputEvent::CursorMoved(from));
    d.input(&mut app, InputEvent::mouse_down(1));
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(from.x - 20.0, from.y)));
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(from.x - 7.0, from.y)));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.resizing.map(|r| (r.w, r.snap)), Some((150.0, Some(150.0))));
    assert!(d.key_of("snap").is_some(), "the line under them while dragged");
    d.input(&mut app, InputEvent::mouse_up());
    d_frame_twice(&mut app, &mut d);
    assert_eq!(width_of(&app, "b"), Some(Some(150)));
    assert_eq!(width_of(&app, "a"), Some(Some(200)), "the other as it was");
    assert!(d.key_of("snap").is_none(), "the line gone once let go");
    // Each picture's node and its row's: both pictures 150 tall.
    let heights: Vec<f32> = d.core.nodes().iter().filter(|n| n.role == Some(Role::Image)).map(|n| n.rect.h).filter(|h| *h < 170.0).collect();
    assert!(heights.len() == 2 && heights.iter().all(|h| (h - 150.0).abs() < 1.0), "lined up: {heights:?}");
    // Further off than the snap: as dragged.
    pick_b(&mut app, &mut d);
    let handle = d.key_of("resize-r").unwrap();
    let from = d.core.nodes().iter().find(|n| n.key == handle).map(|n| n.rect.center()).unwrap();
    d.drag(&mut app, from, Vec2::new(from.x + 30.0, from.y));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(width_of(&app, "b"), Some(Some(180)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dropped_pictures_go_where_the_line_is() {
    let (mut app, mut d, dir) = body("drop-line");
    for (i, s) in ["one", "two", "three"].iter().enumerate() {
        if i > 0 {
            key(&mut app, &mut d, "enter");
        }
        type_in(&mut app, &mut d, s);
    }
    d.frame(&mut app);
    let outside = std::env::temp_dir().join(format!("carnet-drop-line-{}", std::process::id()));
    let pic = png_file(&outside, "shot.png");
    // Over the gap between "one" and "two".
    let r1 = app.row_rects[1].1;
    let at = Vec2::new(r1.x + 40.0, r1.y + 2.0);
    let paths = vec![pic.display().to_string()];
    d.input(&mut app, InputEvent::DragFiles { paths: paths.clone(), at });
    d.frame(&mut app);
    d.frame(&mut app);
    assert_eq!(app.drop_index, Some(1), "the line before \"two\"");
    assert!(d.key_of("drop-line").is_some(), "drawn");
    d.input(&mut app, InputEvent::DropFiles { paths, at });
    d.frame(&mut app);
    assert_eq!(texts(&app)[..4], ["one", "", "two", "three"], "{:?}", texts(&app));
    assert!(matches!(blocks(&app)[1].kind, Kind::Image { .. }));
    assert_eq!(app.drop_index, None);
    assert!(d.key_of("drop-line").is_none(), "gone after the drop");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn resting_a_dragged_file_on_a_note_opens_it() {
    let (mut app, mut d, dir) = fresh("spring");
    d.text(&mut app, "Here");
    d.frame(&mut app);
    app.save_current();
    let here = app.current;
    d.core.set_inspect(true);
    d.frame(&mut app);
    let card = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Welcome to Carnet"))
        .map(|n| n.rect.center())
        .expect("the other note's card");
    let paths = vec!["C:\\nowhere\\x.png".to_string()];
    d.input(&mut app, InputEvent::DragFiles { paths, at: card });
    d.frame(&mut app);
    assert_eq!(app.current, here, "not at once");
    std::thread::sleep(SPRING + Duration::from_millis(50));
    d.frame(&mut app);
    assert_ne!(app.current, here, "after a moment, the note under it");
    assert_eq!(app.store.notes[app.current].title, "Welcome to Carnet");
    d.input(&mut app, InputEvent::DragCancel);
    d.frame(&mut app);
    assert!(app.drop_index.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

const CTRL_ALT: KeyMods = KeyMods { shift: false, ctrl: true, alt: true, super_key: false };
const CTRL_SHIFT: KeyMods = KeyMods { shift: true, ctrl: true, alt: false, super_key: false };

#[test]
fn ctrl_up_and_down_go_to_the_note_edges_and_shift_selects() {
    let (mut app, mut d, dir) = body("edges");
    for (i, s) in ["one", "two", "three"].iter().enumerate() {
        if i > 0 {
            key(&mut app, &mut d, "enter");
        }
        type_in(&mut app, &mut d, s);
    }
    key_with(&mut app, &mut d, "up", CTRL);
    assert_eq!(app.caret.head, page::first(app.doc()), "the top");
    key_with(&mut app, &mut d, "down", CTRL_SHIFT);
    let (a, b) = page::ordered(app.doc(), app.caret);
    assert_eq!(a, page::first(app.doc()));
    assert_eq!(b.slot, Slot::block(blocks(&app).last().unwrap().id), "selected to the bottom");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn block_shortcuts_turn_duplicate_and_remove() {
    let (mut app, mut d, dir) = body("block-keys");
    type_in(&mut app, &mut d, "Plan");
    key_with(&mut app, &mut d, "2", CTRL_ALT);
    assert!(matches!(&blocks(&app)[0].kind, Kind::Heading { level: 2, text } if text == "Plan"));
    key_with(&mut app, &mut d, "4", CTRL_ALT);
    assert!(matches!(blocks(&app)[0].kind, Kind::Item { style: ListStyle::Todo, .. }));
    key_with(&mut app, &mut d, "d", CTRL);
    assert_eq!(texts(&app)[..2], ["[ ]Plan", "[ ]Plan"], "duplicated below");
    key_with(&mut app, &mut d, "k", CTRL_SHIFT);
    assert_eq!(texts(&app)[0], "[ ]Plan");
    assert_ne!(texts(&app).get(1).map(String::as_str), Some("[ ]Plan"), "the copy removed");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_cyrillic_layout_still_copies_with_ctrl_c() {
    let (mut app, mut d, dir) = body("layout");
    type_in(&mut app, &mut d, "привет");
    key_with(&mut app, &mut d, "home", SHIFT);
    // Ctrl+С: the key where C is, typing "с" on a Russian layout.
    let press = kui_native::KeyPress {
        code: KeyCode::Char('с'),
        physical: KeyCode::Char('c'),
        mods: CTRL,
        text: None,
        repeat: false,
        location: Default::default(),
        locks: Default::default(),
    };
    app.page_key_press_for_test(press, &d.core);
    d.frame(&mut app);
    assert_eq!(copied(&mut d).as_deref(), Some("привет"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_note_dragged_onto_a_folder_moves_there() {
    let (mut app, mut d, dir) = fresh("drag-note");
    d.text(&mut app, "Trip");
    d.frame(&mut app);
    app.save_current();
    app.store.create_folder("", "Home").unwrap();
    d.core.set_inspect(true);
    d.frame(&mut app);
    d.frame(&mut app);
    let find = |d: &Drive, s: &str| d.core.nodes().iter().find(|n| n.text.as_deref() == Some(s)).map(|n| n.rect.center());
    let card = find(&d, "Trip").expect("the card");
    let home = find(&d, "Home").expect("the folder row");
    d.drag(&mut app, card, home);
    d.frame(&mut app);
    let i = app.store.notes.iter().position(|n| n.title == "Trip").unwrap();
    assert_eq!(app.store.notes[i].folder, "Home");
    assert!(dir.join("Home").join("Trip.md").exists());
    // A plain click on a card still opens it.
    d.frame(&mut app);
    let welcome = find(&d, "Welcome to Carnet").expect("another card");
    d.click(&mut app, welcome.x, welcome.y);
    d.frame(&mut app);
    assert_eq!(app.store.notes[app.current].title, "Welcome to Carnet");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ctrl_alt_down_opens_the_next_note() {
    let (mut app, mut d, dir) = body("next-note");
    type_in(&mut app, &mut d, "x");
    app.store.notes[app.current].title = "First".into();
    app.save_current();
    let first = app.current;
    key_with(&mut app, &mut d, "down", CTRL_ALT);
    assert_ne!(app.current, first);
    key_with(&mut app, &mut d, "up", CTRL_ALT);
    assert_eq!(app.current, first);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_numbered_list_broken_by_bullets_keeps_its_numbers() {
    let (mut app, mut d, dir) = body("numbers");
    app.store.notes[app.current].doc = Doc::new(carnet_core::doc::parse_blocks("1. one\n\n- a\n\n2. two"));
    app.caret = Caret::at(page::first(app.doc()));
    d.core.set_inspect(true);
    d.frame(&mut app);
    d.frame(&mut app);
    let shown = |s: &str| d.core.nodes().iter().any(|n| n.text.as_deref() == Some(s));
    assert!(shown("1.") && shown("2."), "the second item reads 2., as written");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_narrow_strip_folds_tools_into_more() {
    let (store, dir) = temp_store("more");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 620.0, 600.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    app.edit_at(page::first(app.doc()));
    for _ in 0..3 {
        d.frame(&mut app);
    }
    assert!(d.key_of("t-more").is_some(), "a More button");
    // The last group (the block's own tools) goes first, whole.
    assert!(d.key_of("t-up").is_none() && d.key_of("t-down").is_none(), "folded into it");
    assert!(d.key_of("t-text").is_some(), "the first group stays");
    let before = texts(&app);
    let more = d.key_of("t-more").unwrap();
    d.click_key(&mut app, more);
    d.frame(&mut app);
    d.frame(&mut app);
    let item = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some("Move down"))
        .map(|n| n.rect.center())
        .expect("the block's tools in the menu");
    d.click(&mut app, item.x, item.y);
    d.frame(&mut app);
    assert_eq!(texts(&app)[1], before[0], "the block moved down from the menu");
    // Wide, everything shows and there is no More.
    let mut wide = Drive::new(Core::new(), 1400.0, 800.0).framing();
    wide.core.set_inspect(true);
    for _ in 0..3 {
        wide.frame(&mut app);
    }
    assert!(wide.key_of("t-more").is_none());
    assert!(wide.key_of("t-up").is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn f12_opens_the_devtools_and_shift_f12_the_hud() {
    let (mut app, mut d, dir) = body("f12");
    key(&mut app, &mut d, "f12");
    assert_eq!(d.core.devtools(), cfg!(debug_assertions));
    key(&mut app, &mut d, "f12");
    assert!(!d.core.devtools());
    key_with(&mut app, &mut d, "f12", SHIFT);
    assert!(app.hud);
    key_with(&mut app, &mut d, "f12", SHIFT);
    assert!(!app.hud);
    let _ = std::fs::remove_dir_all(dir);
}

/// The drawn text of the line node holding `slot`.
fn drawn_text(app: &Carnet, d: &Drive, slot: Slot) -> String {
    let k = *app.slot_keys.get(&slot).expect("the line");
    d.core.nodes().iter().find(|n| n.key == k).and_then(|n| n.text.clone()).unwrap_or_default()
}

/// Where the caret is in the drawn words of its line.
fn drawn_caret(app: &Carnet) -> usize {
    page::visible(app.doc(), app.caret.head.slot).drawn(app.caret.head.byte)
}

/// A click on the drawn words of `slot`, just right of place `d`.
fn click_drawn(app: &mut Carnet, d: &mut Drive, slot: Slot, at: usize) {
    let k = *app.slot_keys.get(&slot).unwrap();
    let r = d.core.caret_rect(k, at).unwrap();
    d.click(app, r.x + 1.0, r.y + r.h / 2.0);
    d.frame(app);
    d.frame(app);
}

#[test]
fn marks_are_never_drawn_and_the_caret_moves_by_the_words() {
    let (mut app, mut d, dir) = body("marks");
    d.core.set_inspect(true);
    let line = "a **bold** ==word== [link](http://x) end";
    let words = "a bold word link end";
    type_in(&mut app, &mut d, line);
    let first = app.caret.head.slot;
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "next");
    d.frame(&mut app);
    assert_eq!(drawn_text(&app, &d, first), words, "off the caret's line");
    key(&mut app, &mut d, "up");
    d.frame(&mut app);
    assert_eq!(app.caret.head.slot, first);
    assert_eq!(drawn_text(&app, &d, first), words, "and on it");

    // Every arrow press moves the caret one letter of the words: never a
    // press spent stepping over a hidden mark.
    key(&mut app, &mut d, "home");
    assert_eq!(drawn_caret(&app), 0);
    for i in 1..=words.len() {
        key(&mut app, &mut d, "right");
        assert_eq!(drawn_caret(&app), i, "right #{i}");
    }
    for i in (0..words.len()).rev() {
        key(&mut app, &mut d, "left");
        assert_eq!(drawn_caret(&app), i, "left to {i}");
    }
    // After a styled word's last letter, the caret is inside its marks:
    // typing carries the style on.
    for _ in 0.."a bold".len() {
        key(&mut app, &mut d, "right");
    }
    assert_eq!(app.caret.head.byte, "a **bold".len());
    // Word by word.
    key(&mut app, &mut d, "home");
    key_with(&mut app, &mut d, "right", CTRL);
    key_with(&mut app, &mut d, "right", CTRL);
    assert_eq!(drawn_caret(&app), "a bold".len());

    // A click anywhere in the words puts the caret there.
    for at in [0, 2, 3, 6, 7, 11, 12, 16, words.len()] {
        click_drawn(&mut app, &mut d, first, at);
        assert_eq!((app.caret.head.slot, drawn_caret(&app)), (first, at), "a click at {at}");
    }
    // Shift+arrows select by the words; the selection copies whole marks.
    click_drawn(&mut app, &mut d, first, 4);
    for _ in 0..5 {
        key_with(&mut app, &mut d, "right", SHIFT);
    }
    key_with(&mut app, &mut d, "c", CTRL);
    assert_eq!(copied(&mut d).as_deref(), Some("**ld** ==wo=="));
    // Backspace after "bold" takes its d, not a star.
    click_drawn(&mut app, &mut d, first, 6);
    key(&mut app, &mut d, "backspace");
    assert_eq!(texts(&app)[0], "a **bol** ==word== [link](http://x) end");
    assert_eq!(drawn_caret(&app), 5);
    // Down and up again land under the same letter.
    click_drawn(&mut app, &mut d, first, 3);
    key(&mut app, &mut d, "down");
    key(&mut app, &mut d, "up");
    assert_eq!((app.caret.head.slot, drawn_caret(&app)), (first, 3));
    // Enter inside a bold word: bold on both lines.
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[..2], ["a **b**", "**ol** ==word== [link](http://x) end"]);
    // Backspace at the second line's start joins them back, one bold run.
    key(&mut app, &mut d, "backspace");
    assert_eq!(texts(&app)[0], "a **bol** ==word== [link](http://x) end");
    assert_eq!(drawn_caret(&app), 3);
    // Taking a styled word's last letter takes its marks with it.
    click_drawn(&mut app, &mut d, first, 5);
    key_with(&mut app, &mut d, "backspace", CTRL);
    assert_eq!(texts(&app)[0], "a  ==word== [link](http://x) end");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_style_with_nothing_selected_is_for_what_is_typed_next() {
    let (mut app, mut d, dir) = body("pending");
    type_in(&mut app, &mut d, "plain ");
    key_with(&mut app, &mut d, "b", CTRL);
    type_in(&mut app, &mut d, "bold");
    type_in(&mut app, &mut d, "er");
    key_with(&mut app, &mut d, "b", CTRL);
    type_in(&mut app, &mut d, " after");
    assert_eq!(texts(&app)[0], "plain **bolder** after");
    // Twice is off again.
    key_with(&mut app, &mut d, "i", CTRL);
    key_with(&mut app, &mut d, "i", CTRL);
    type_in(&mut app, &mut d, "!");
    assert_eq!(texts(&app)[0], "plain **bolder** after!");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_space_typed_in_bold_never_breaks_the_run() {
    let (mut app, mut d, dir) = body("bold-space");
    d.core.set_inspect(true);
    let chord = if cfg!(target_os = "macos") { CMD } else { CTRL };
    key_with(&mut app, &mut d, "b", chord);
    let slot = app.caret.head.slot;
    // A key at a time: `**Bold **` would draw its marks until the next
    // letter came.
    for ch in "Bold here".chars() {
        type_in(&mut app, &mut d, &ch.to_string());
        d.frame(&mut app);
        let drawn = drawn_text(&app, &d, slot);
        assert!(!drawn.contains('*'), "after {ch:?}: {drawn:?} ({:?})", texts(&app)[0]);
    }
    assert_eq!(drawn_text(&app, &d, slot), "Bold here");
    app.store.notes[app.current].title = "Bold".into();
    app.save_current();
    let md = std::fs::read_to_string(app.store.notes[app.current].path.clone().unwrap()).unwrap();
    assert!(md.contains("\n**Bold here**\n"), "{md}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_link_is_made_and_edited_in_its_editor() {
    let (mut app, mut d, dir) = body("link");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "see docs");
    let slot = app.caret.head.slot;
    key_with(&mut app, &mut d, "k", CTRL);
    d.frame(&mut app);
    d.frame(&mut app);
    assert!(app.link_edit.as_ref().is_some_and(|l| !l.existing), "the editor, for the word at the caret");
    type_in(&mut app, &mut d, "http://x.test/a b");
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[0], "see [docs](http://x.test/a%20b)");
    assert!(app.link_edit.is_none());
    d.frame(&mut app);
    assert_eq!(drawn_text(&app, &d, slot), "see docs");
    // In the link, Ctrl+K edits it.
    click_drawn(&mut app, &mut d, slot, 6);
    key_with(&mut app, &mut d, "k", CTRL);
    d.frame(&mut app);
    assert_eq!(app.link_edit.as_ref().map(|l| (l.existing, l.url.as_str())), Some((true, "http://x.test/a%20b")));
    app.link_act(2);
    assert_eq!(texts(&app)[0], "see docs", "Remove link keeps the words");
    let _ = std::fs::remove_dir_all(dir);
}

/// The link editor opened again, on another link or on none, shows that
/// one's address: not what the field held when it was last put away.
#[test]
fn the_link_editor_shows_the_link_it_was_opened_on() {
    let (mut app, mut d, dir) = body("link-again");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "one two");
    let slot = app.caret.head.slot;
    key_with(&mut app, &mut d, "k", CTRL);
    d.frame(&mut app);
    type_in(&mut app, &mut d, "https://x.test/two");
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[0], "one [two](https://x.test/two)");
    d.frame(&mut app);
    let field = |d: &mut Drive| {
        let k = d.key_of("link-url").expect("the link's field");
        d.core.edit_text(k).unwrap_or_default()
    };
    // On a word with no link: an empty field.
    click_drawn(&mut app, &mut d, slot, 1);
    key_with(&mut app, &mut d, "k", CTRL);
    d.frame(&mut app);
    d.frame(&mut app);
    assert_eq!(field(&mut d), "");
    key(&mut app, &mut d, "escape");
    d.frame(&mut app);
    // On the link: its address.
    click_drawn(&mut app, &mut d, slot, 5);
    key_with(&mut app, &mut d, "k", CTRL);
    d.frame(&mut app);
    d.frame(&mut app);
    assert_eq!(field(&mut d), "https://x.test/two");
    // Its undo starts with it: Ctrl+Z (the field's Undo, as the window
    // hands it on) goes back into no other address.
    d.input(&mut app, InputEvent::Key(EditKey::Undo, kui_native::Mods::default()));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(field(&mut d), "https://x.test/two");
    assert_eq!(app.link_edit.as_ref().map(|l| l.url.as_str()), Some("https://x.test/two"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_link_typed_without_a_scheme_is_given_the_webs() {
    let (mut app, mut d, dir) = body("link-scheme");
    type_in(&mut app, &mut d, "see ");
    key_with(&mut app, &mut d, "k", CTRL);
    d.frame(&mut app);
    type_in(&mut app, &mut d, "x.test/a");
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[0], "see [x.test/a](https://x.test/a)", "the words as typed, the address with its scheme");
    let _ = std::fs::remove_dir_all(dir);
}

/// Ctrl+click (⌘ on a Mac) on a link opens only one a note may follow:
/// never one that runs (`javascript:`), nor a mail that would attach a
/// file of this computer's, however it is written. A mail's address
/// alone (`<me@x.test>`) opens a mail to it.
#[test]
fn a_ctrl_click_opens_only_a_link_a_note_may_follow() {
    let (mut app, mut d, dir) = body("link-open");
    d.core.set_inspect(true);
    let lines = [
        "[mail](mailto:a@x.test?attach=/home/u/.ssh/id_ed25519)",
        "[run](javascript:alert(1))",
        "[bare](me@x.test?%41ttachment=a.txt)",
        "[web](https://x.test/a)",
        "<me@x.test>",
    ];
    let mut slots = Vec::new();
    for line in lines {
        type_in(&mut app, &mut d, line);
        slots.push(app.caret.head.slot);
        key(&mut app, &mut d, "enter");
    }
    assert_eq!(texts(&app)[..5], lines);
    let ctrl_click = |app: &mut Carnet, d: &mut Drive, slot: Slot| {
        d.input(app, InputEvent::Modifiers(CTRL));
        click_drawn(app, d, slot, 1);
        d.input(app, InputEvent::Modifiers(KeyMods::NONE));
        d.frame(app);
    };
    let opened = || store::OPENED.with_borrow_mut(std::mem::take);
    for (slot, line) in slots[..3].iter().zip(lines) {
        ctrl_click(&mut app, &mut d, *slot);
        assert_eq!(opened(), Vec::<String>::new(), "{line} opens nothing");
        assert!(app.error.take().is_some(), "{line}: and says so");
    }
    ctrl_click(&mut app, &mut d, slots[3]);
    assert_eq!(opened(), ["https://x.test/a"]);
    assert_eq!(app.error, None);
    // A mail's address alone opens a mail to it.
    ctrl_click(&mut app, &mut d, slots[4]);
    assert_eq!(opened(), ["mailto:me@x.test"]);
    assert_eq!(app.error, None);
    // The menu's Open link and the link editor's go the same way.
    click_drawn(&mut app, &mut d, slots[0], 2);
    app.page_action("open-link");
    assert_eq!(opened(), Vec::<String>::new(), "from the menu");
    key_with(&mut app, &mut d, "k", CTRL);
    d.frame(&mut app);
    assert!(app.link_edit.as_ref().is_some_and(|l| l.url.starts_with("mailto:")));
    app.link_act(1);
    assert_eq!(opened(), Vec::<String>::new(), "from the link editor");
    key(&mut app, &mut d, "escape");
    click_drawn(&mut app, &mut d, slots[3], 2);
    app.page_action("open-link");
    assert_eq!(opened(), ["https://x.test/a"], "and a web one opens from the menu");
    let _ = std::fs::remove_dir_all(dir);
}
/// Links open as Settings says: ⌘-click (Ctrl elsewhere, the default),
/// a click, or a double-click. The press that opens one leaves the caret
/// where it was; any other places it, so a link's words can always be
/// edited: with a click opening, a drag or a Shift-click selects, a
/// ⌘-click places the caret, and so does a click at the words' end. A
/// link that would run opens by none. The pointer is a hand over a link
/// just when a press there would open it.
#[test]
fn links_open_as_the_settings_say() {
    use page::LinkOpen;
    let (mut app, mut d, dir) = body("link-modes");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "see [docs](https://x.test/d) end [run](javascript:alert(1))");
    let slot = app.caret.head.slot;
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "below");
    let below = app.caret;
    let opened = || store::OPENED.with_borrow_mut(std::mem::take);
    let with = |app: &mut Carnet, d: &mut Drive, mods: KeyMods, f: &dyn Fn(&mut Carnet, &mut Drive)| {
        d.input(app, InputEvent::Modifiers(mods));
        f(app, d);
        d.input(app, InputEvent::Modifiers(KeyMods::NONE));
        d.frame(app);
    };
    let point = |app: &Carnet, d: &Drive, at: usize| {
        let r = d.core.caret_rect(*app.slot_keys.get(&slot).unwrap(), at).unwrap();
        Vec2::new(r.x + 1.0, r.y + r.h / 2.0)
    };
    let in_link = |app: &Carnet| page::link_at(app.doc(), app.caret).is_some();
    let hand = |app: &mut Carnet, d: &mut Drive, at: usize, mods: KeyMods| {
        let p = point(app, d, at);
        d.input(app, InputEvent::Modifiers(mods));
        d.move_to(app, p.x, p.y);
        d.frame(app);
        let shape = d.core.cursor_shape();
        d.input(app, InputEvent::Modifiers(KeyMods::NONE));
        d.frame(app);
        shape == kui_native::CursorShape::Pointer
    };

    // ⌘-click, as ever: a plain click places the caret in the link.
    assert_eq!(app.settings.link_open, LinkOpen::ModClick);
    click_drawn(&mut app, &mut d, slot, 6);
    assert_eq!(opened(), Vec::<String>::new());
    assert!(in_link(&app));
    with(&mut app, &mut d, CTRL, &|app, d| click_drawn(app, d, slot, 6));
    assert_eq!(opened(), ["https://x.test/d"]);
    assert!(!hand(&mut app, &mut d, 6, KeyMods::NONE), "no hand with no key held");
    assert!(hand(&mut app, &mut d, 6, CTRL), "a hand while it's held");
    assert!(!hand(&mut app, &mut d, 2, CTRL), "not over plain words");

    // A click.
    app.settings.link_open = LinkOpen::Click;
    app.caret = below;
    d.frame(&mut app);
    click_drawn(&mut app, &mut d, slot, 6);
    assert_eq!(opened(), ["https://x.test/d"]);
    assert_eq!(app.caret, below, "the caret left where it was");
    assert_eq!(app.error, None);
    // ⌘-click places the caret in it, to edit its words.
    with(&mut app, &mut d, CTRL, &|app, d| click_drawn(app, d, slot, 6));
    assert_eq!(opened(), Vec::<String>::new());
    assert!(in_link(&app) && app.caret.is_empty());
    assert_eq!(drawn_caret(&app), 6);
    // A click at its words' end places the caret there.
    click_drawn(&mut app, &mut d, slot, 8);
    assert_eq!(opened(), Vec::<String>::new());
    assert_eq!(drawn_caret(&app), 8);
    // A drag across it selects.
    let (a, b) = (point(&app, &d, 5), point(&app, &d, 7));
    d.drag(&mut app, a, b);
    d.frame(&mut app);
    assert_eq!(opened(), Vec::<String>::new());
    assert!(!app.caret.is_empty(), "a selection");
    // Shift-click: the selection to it.
    app.caret = Caret::at(Pos { slot, byte: 0 });
    d.frame(&mut app);
    with(&mut app, &mut d, SHIFT, &|app, d| click_drawn(app, d, slot, 6));
    assert_eq!(opened(), Vec::<String>::new());
    assert!(!app.caret.is_empty());
    // One that would run: a click places the caret, and says nothing.
    let run = "see docs end r".len();
    click_drawn(&mut app, &mut d, slot, run);
    assert_eq!(opened(), Vec::<String>::new());
    assert_eq!(app.error, None);
    assert!(in_link(&app));
    assert!(hand(&mut app, &mut d, 6, KeyMods::NONE), "a hand over a link");
    assert!(!hand(&mut app, &mut d, 6, CTRL), "not with ⌘, which places the caret");
    assert!(!hand(&mut app, &mut d, 8, KeyMods::NONE), "not at its end");
    assert!(!hand(&mut app, &mut d, run, KeyMods::NONE), "not over one that won't open");

    // A double-click, in place of choosing the word; one click places
    // the caret.
    app.settings.link_open = LinkOpen::DoubleClick;
    app.caret = below;
    d.frame(&mut app);
    click_drawn(&mut app, &mut d, slot, 6);
    assert_eq!(opened(), Vec::<String>::new());
    assert!(in_link(&app));
    let p = point(&app, &d, 6);
    d.double_click(&mut app, p.x, p.y);
    d.frame(&mut app);
    assert_eq!(opened(), ["https://x.test/d"]);
    assert!(app.caret.is_empty(), "no word chosen");
    // A third press, a triple-click's: nothing — the line isn't chosen
    // (the finding: it was, once the link had opened), nor the link
    // opened again.
    let caret = app.caret;
    let p = point(&app, &d, 6);
    d.double_click(&mut app, p.x, p.y);
    d.input(&mut app, InputEvent::mouse_down(3));
    d.input(&mut app, InputEvent::mouse_up());
    d.frame(&mut app);
    assert_eq!(opened(), ["https://x.test/d"], "opened once");
    assert_eq!(app.caret, caret, "nothing chosen");
    // Elsewhere two clicks still choose a word, and three the line.
    let p = point(&app, &d, 1);
    d.double_click(&mut app, p.x, p.y);
    d.frame(&mut app);
    assert_eq!(opened(), Vec::<String>::new());
    assert!(!app.caret.is_empty());
    d.input(&mut app, InputEvent::mouse_down(3));
    d.input(&mut app, InputEvent::mouse_up());
    d.frame(&mut app);
    assert_eq!(page::ordered(app.doc(), app.caret).1.byte, page::text(app.doc(), slot).len(), "the line chosen");
    assert!(hand(&mut app, &mut d, 6, KeyMods::NONE));
    let _ = std::fs::remove_dir_all(dir);
}

/// A bare address (`https://…`, `www.…`, a mail's), as GitHub links one:
/// drawn as a link and opened as one, the note's words as they were.
#[test]
fn a_bare_address_is_a_link_on_the_page() {
    let (mut app, mut d, dir) = body("bare-link");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "see www.x.test/d, or me@y.test.");
    let slot = app.caret.head.slot;
    let point = |app: &Carnet, d: &Drive, at: usize| {
        let r = d.core.caret_rect(*app.slot_keys.get(&slot).unwrap(), at).unwrap();
        Vec2::new(r.x + 1.0, r.y + r.h / 2.0)
    };
    let opened = || store::OPENED.with_borrow_mut(std::mem::take);
    assert!(page::style_at(app.doc(), Caret::at(Pos { slot, byte: 7 })).link, "drawn as a link");
    for (at, url) in [(7, "https://www.x.test/d"), ("see www.x.test/d, or m".len(), "mailto:me@y.test")] {
        let p = point(&app, &d, at);
        d.input(&mut app, InputEvent::Modifiers(CTRL));
        d.click(&mut app, p.x, p.y);
        d.input(&mut app, InputEvent::Modifiers(KeyMods::NONE));
        d.frame(&mut app);
        assert_eq!(opened(), [url]);
    }
    assert_eq!(texts(&app)[0], "see www.x.test/d, or me@y.test.");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_colour_picker_colours_the_selection() {
    let (mut app, mut d, dir) = body("tint");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "red words");
    let slot = app.caret.head.slot;
    app.caret = Caret { anchor: Pos { slot, byte: 0 }, head: Pos { slot, byte: 3 } };
    d.frame(&mut app);
    let tool = d.key_of("t-color").expect("the text colour tool");
    d.click_key(&mut app, tool);
    d.frame(&mut app);
    d.frame(&mut app);
    // The swatches are named for their hues.
    let red = d.key_named("Red").expect("the red swatch");
    d.click_key(&mut app, red);
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "<span style=\"color: #C0553F\">red</span> words");
    assert!(app.tint_open.is_some(), "the picker stays, for the other colour");
    assert!(d.key_named("Recent").is_none(), "and no row of colours given lately");
    key(&mut app, &mut d, "escape");
    assert!(app.tint_open.is_none(), "Esc puts it away");
    assert!(page_focused(&app, &d), "the page keeps the keyboard");
    // Ctrl+Shift+H: the plain highlighter, Obsidian's ==.
    key_with(&mut app, &mut d, "h", CTRL_SHIFT);
    // The highlighter joins the colour in its one tag.
    assert_eq!(texts(&app)[0], "<span style=\"color: #C0553F; background: #FFF3A3A6\">red</span> words");
    key(&mut app, &mut d, "end");
    d.frame(&mut app);
    assert_eq!(drawn_text(&app, &d, slot), "red words", "no tag drawn, even on the caret's line");
    click_drawn(&mut app, &mut d, slot, 2);
    assert_eq!(drawn_caret(&app), 2, "a click maps through the hidden tag");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_quote_is_typed_and_stepped_out_of() {
    let (mut app, mut d, dir) = body("quote");
    type_in(&mut app, &mut d, "> ");
    assert!(matches!(blocks(&app)[0].kind, Kind::Quote(_)), "{:?}", texts(&app));
    type_in(&mut app, &mut d, "line");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "two");
    assert_eq!(texts(&app)[0], "line\ntwo", "Enter goes on a line at a time");
    key(&mut app, &mut d, "enter");
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[0], "line\ntwo", "an empty last line steps out");
    assert!(matches!(blocks(&app)[1].kind, Kind::Text(_)));
    // Ctrl+Alt+7 turns a paragraph into one, and Backspace at its start back.
    type_in(&mut app, &mut d, "said");
    key_with(&mut app, &mut d, "7", CTRL_ALT);
    assert!(matches!(&blocks(&app)[1].kind, Kind::Quote(t) if t == "said"));
    key(&mut app, &mut d, "home");
    key(&mut app, &mut d, "backspace");
    assert!(matches!(&blocks(&app)[1].kind, Kind::Text(t) if t == "said"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_picture_at_the_right_has_the_words_beside_it() {
    let (mut app, mut d, dir) = with_picture("float");
    key_with(&mut app, &mut d, "r", CTRL_ALT);
    let pic_id = blocks(&app).iter().find(|b| matches!(b.kind, Kind::Image { .. })).unwrap().id;
    assert!(matches!(app.doc().get(pic_id).unwrap().kind, Kind::Image { float: Float::Right, .. }));
    // Words after it.
    let tail = blocks(&app).last().unwrap().id;
    app.caret = Caret::at(page::start(Slot::block(tail)));
    type_in(&mut app, &mut d, "Beside the picture");
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let rect = |id: u64| app.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r).unwrap();
    let (pic, words) = (rect(pic_id), rect(tail));
    assert!(words.y < pic.y + pic.h && words.x + words.w <= pic.x + 1.0, "words {words:?} left of picture {pic:?}");
    // A click on them puts the caret in them, not on the picture.
    d.click(&mut app, words.x + 20.0, words.y + words.h / 2.0);
    d.frame(&mut app);
    assert_eq!(app.caret.head.slot, Slot::block(tail));
    // Its left side widens it, going left.
    let i = app.doc().index_of(pic_id).unwrap();
    app.caret = page::select_blocks(app.doc(), i, i);
    d.frame(&mut app);
    d.frame(&mut app);
    let before = app.pic_w[&pic_id].0;
    let handle = d.key_of("resize-l").expect("a handle on its left side");
    let from = d.core.nodes().iter().find(|n| n.key == handle).unwrap().rect.center();
    d.drag(&mut app, from, Vec2::new(from.x - 60.0, from.y));
    d.frame(&mut app);
    let w = image_width(&app).flatten().expect("a width") as f32;
    assert!(w > before + 40.0, "{before} -> {w}");
    // Sizes from its menu, and back on its own line.
    app.page_action("size:50");
    let half = ((app.text_w - 10.0) * 0.5).round() as u32;
    assert_eq!(image_width(&app), Some(Some(half)));
    app.page_action("float:0");
    assert!(matches!(app.doc().get(pic_id).unwrap().kind, Kind::Image { float: Float::None, .. }));
    app.page_action("float:1");
    app.store.notes[app.current].title = "Float".into();
    app.save_current();
    let md = std::fs::read_to_string(app.store.notes[app.current].path.clone().unwrap()).unwrap();
    assert!(md.contains(&format!("|left|{half}](attachments/")), "{md}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: a picture's resize handle pressed and let go, no drag,
/// dropped its `|240`: the row's share drew it narrower than its own 240,
/// and the width it was let go at was taken for "none". A press changes
/// nothing; a drag writes the width chosen.
#[test]
fn a_resize_handle_pressed_and_let_go_changes_nothing() {
    let body = "![a|left|240](attachments/a.png)\n\n![b|left|240](attachments/b.png)\n\n![c|left|240](attachments/c.png)\n\nWords";
    let (mut app, mut d, dir) = library("pic-press", &[("Row", body)]);
    let rgba: Vec<u8> = (0..240 * 180).flat_map(|_| [94u8, 124, 164, 255]).collect();
    let png = crate::images::encode_png(240, 180, &rgba).unwrap();
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    for n in ["a", "b", "c"] {
        std::fs::write(dir.join("attachments").join(format!("{n}.png")), &png).unwrap();
    }
    app.store.index_files();
    app.images.retry_failed();
    app.open(app.store.notes.iter().position(|n| n.title == "Row").unwrap());
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let id = blocks(&app).iter().find(|b| matches!(b.kind, Kind::Image { .. })).unwrap().id;
    let i = app.doc().index_of(id).unwrap();
    app.caret = page::select_blocks(app.doc(), i, i);
    d_frame_twice(&mut app, &mut d);
    let drawn = app.pic_w[&id].0;
    assert!(drawn < 239.0, "the row's share draws it narrower than its own: {drawn}");
    let width = |app: &Carnet| match &app.doc().get(id).unwrap().kind {
        Kind::Image { width, .. } => *width,
        _ => None,
    };
    for handle in ["resize", "resize-l", "resize-r"] {
        let k = d.key_of(handle).unwrap_or_else(|| panic!("{handle}"));
        let at = d.core.nodes().iter().find(|n| n.key == k).unwrap().rect.center();
        d.click(&mut app, at.x, at.y);
        d_frame_twice(&mut app, &mut d);
        assert_eq!(width(&app), Some(240), "{handle}: pressed, nothing changed");
    }
    // Dragged narrower: that width, written.
    let k = d.key_of("resize").unwrap();
    let at = d.core.nodes().iter().find(|n| n.key == k).unwrap().rect.center();
    d.drag(&mut app, at, Vec2::new(at.x - 40.0, at.y));
    d_frame_twice(&mut app, &mut d);
    let w = width(&app).expect("a width");
    assert!((w as f32 - (drawn - 40.0)).abs() <= 2.0, "{drawn} -> {w}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: `![a|left|200](x)![b|left|200](y)` on one line showed as
/// words ("!a|left|200"). It is a row of two pictures, and the file stays
/// as it is until the note is edited.
#[test]
fn pictures_on_one_line_are_a_row_of_pictures() {
    let line = "![a|left|200](attachments/a.png)![b|left|200](attachments/b.png)";
    let (mut app, mut d, dir) = library("pic-line", &[("Line", &format!("{line}\n\nWords beside them"))]);
    png_file(&dir.join("attachments"), "a.png");
    png_file(&dir.join("attachments"), "b.png");
    app.store.index_files();
    app.images.retry_failed();
    app.open(app.store.notes.iter().position(|n| n.title == "Line").unwrap());
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let pics: Vec<u64> = blocks(&app).iter().filter(|b| matches!(b.kind, Kind::Image { float: Float::Left, width: Some(200), .. })).map(|b| b.id).collect();
    assert_eq!(pics.len(), 2);
    assert!(pics.iter().all(|id| app.pic_w.contains_key(id)), "both drawn as pictures");
    assert!(!d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.contains("left|200"))), "no marks drawn");
    app.save_current();
    let md = std::fs::read_to_string(dir.join("Line.md")).unwrap();
    assert_eq!(md, format!("# Line\n\n{line}\n\nWords beside them\n"), "the file as it was");
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: "Appearance and writing reset" and "Moved … to …" stayed
/// for minutes across notes, held while the window was in the background.
/// A notice about the note goes when another note is opened; one about
/// the whole app stays; and a window in the background holds one two
/// minutes at most.
#[test]
fn a_notice_goes_with_its_note_and_is_held_two_minutes_at_most() {
    let (mut app, mut d, dir) = library("notice-scope", &[("One", "a"), ("Two", "b")]);
    let one = app.store.notes.iter().position(|n| n.title == "One").unwrap();
    let two = app.store.notes.iter().position(|n| n.title == "Two").unwrap();
    app.open(one);
    d_frame_twice(&mut app, &mut d);
    // About the note: gone once another is open.
    app.notify("Moved \u{201c}One\u{201d} to Work", None);
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_some());
    app.open(two);
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_none(), "gone with its note");
    // About the whole app: it stays.
    app.notify_wide("Appearance and writing reset");
    d_frame_twice(&mut app, &mut d);
    app.open(one);
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_some(), "stays across notes");
    // In the background: held, but two minutes at most.
    d.core.env.focused = false;
    let before = std::time::Instant::now() - std::time::Duration::from_secs(60);
    if let Some(n) = app.notice.as_mut() {
        n.came = before;
        n.at = before;
    }
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_some(), "held a minute on");
    let long = std::time::Instant::now() - std::time::Duration::from_secs(150);
    if let Some(n) = app.notice.as_mut() {
        n.came = long;
        n.at = long;
    }
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_none(), "not held past two minutes");
    d.core.env.focused = true;
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn all_notes_has_its_own_menu() {
    let (mut app, mut d, dir) = fresh("all-menu");
    type_in(&mut app, &mut d, "Kept");
    let names: Vec<String> = app.folder_menu("").iter().map(|i| format!("{i:?}")).collect();
    assert!(
        names.iter().any(|n| n.contains("New note")) && names.iter().any(|n| n.contains("Sort by") && n.contains("Date created")),
        "{names:?}"
    );
    app.folder_action("", "sort-title");
    assert_eq!(app.settings.sort, crate::settings::Sort::Title);
    let n = app.store.notes.len();
    app.folder_action("", "new-note");
    d.frame(&mut app);
    assert_eq!(app.store.notes.len(), n + 1);
    assert_eq!(app.store.notes[app.current].folder, "");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn hidden_marks_at_line_edges_in_cells_and_other_scripts() {
    let (mut app, mut d, dir) = body("marks-edges");
    d.core.set_inspect(true);
    // Marks at both ends, a long colour tag, and Cyrillic in bold.
    let line = "**Жирный** и <span style=\"color: #C0504D; background: #FFFF00\">цвет</span> **конец**";
    let words = "Жирный и цвет конец";
    type_in(&mut app, &mut d, line);
    let slot = app.caret.head.slot;
    d.frame(&mut app);
    assert_eq!(drawn_text(&app, &d, slot), words);
    key(&mut app, &mut d, "home");
    assert_eq!(drawn_caret(&app), 0);
    // Right, letter by letter, in a script of two-byte letters.
    let mut seen = vec![0];
    for _ in 0..words.chars().count() {
        key(&mut app, &mut d, "right");
        seen.push(drawn_caret(&app));
    }
    let expected: Vec<usize> = std::iter::once(0).chain(words.char_indices().map(|(i, c)| i + c.len_utf8())).collect();
    assert_eq!(seen, expected);
    // End is after the last letter, inside the bold: typing stays bold.
    key(&mut app, &mut d, "home");
    key(&mut app, &mut d, "end");
    type_in(&mut app, &mut d, "!");
    assert!(texts(&app)[0].ends_with("**конец!**"), "{}", texts(&app)[0]);
    // Clicks on each side of the colour tag.
    let before = "Жирный и ".len();
    for at in [before, before + "ц".len(), before + "цвет".len()] {
        click_drawn(&mut app, &mut d, slot, at);
        assert_eq!(drawn_caret(&app), at, "a click at {at}");
    }
    // A drag from inside the bold word to inside the colour, then Delete:
    // what is left of each keeps its style.
    let k = *app.slot_keys.get(&slot).unwrap();
    let from = d.core.caret_rect(k, "Жир".len()).unwrap();
    let to = d.core.caret_rect(k, before + "цв".len()).unwrap();
    d.drag(&mut app, Vec2::new(from.x + 1.0, from.y + from.h / 2.0), Vec2::new(to.x + 1.0, to.y + to.h / 2.0));
    d.frame(&mut app);
    key(&mut app, &mut d, "delete");
    assert_eq!(
        texts(&app)[0],
        "**Жир**<span style=\"color: #C0504D; background: #FFFF00\">ет</span> **конец!**"
    );
    assert_eq!(drawn_text(&app, &d, slot), "Жирет конец!");
    // Undo puts it all back.
    key_with(&mut app, &mut d, "z", CTRL);
    assert!(texts(&app)[0].starts_with("**Жирный** и <span"));

    // In a table cell, the same.
    key(&mut app, &mut d, "end");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "/table");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "a **b** c");
    let cell = app.caret.head.slot;
    assert!(cell.cell.is_some());
    d.frame(&mut app);
    assert_eq!(drawn_text(&app, &d, cell), "a b c");
    click_drawn(&mut app, &mut d, cell, 3);
    assert_eq!((app.caret.head.slot, drawn_caret(&app)), (cell, 3));
    key(&mut app, &mut d, "backspace");
    assert_eq!(page::text(app.doc(), cell), "a  c", "the b went with its marks");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_folder_with_notes_goes_to_the_trash_whole_and_comes_back() {
    let (mut app, mut d, dir) = fresh("trash-folder");
    let folder = app.store.create_folder("", "Q3").unwrap();
    app.show_folder(Some(folder.clone()));
    app.new_note();
    type_in(&mut app, &mut d, "Plans");
    app.save_current();
    let names = |app: &Carnet| app.folder_menu(&folder).iter().map(|i| format!("{i:?}")).collect::<Vec<_>>().join(" ");
    assert!(names(&app).contains("Move folder to Deleted"), "{}", names(&app));
    app.folder_action(&folder, "delete");
    d.frame(&mut app);
    assert!(!app.store.folders.contains(&folder));
    assert!(app.store.notes.iter().all(|n| n.title != "Plans"));
    assert!(!app.store.folder_dir(&folder).exists());
    assert!(app.notice.as_ref().is_some_and(|n| n.text.contains("its note")), "says what went");
    app.notice_undo();
    assert!(app.store.folders.contains(&folder));
    assert!(app.store.notes.iter().any(|n| n.title == "Plans" && n.folder == folder));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_selected_picture_has_its_place_in_the_toolbar() {
    let (mut app, mut d, dir) = with_picture("float-tools");
    let tool = d.key_of("t-right").expect("the picture's place among the tools");
    assert!(d.key_of("t-bold").is_none() && d.key_of("t-h1").is_none(), "styles do nothing to a picture");
    assert!(d.key_of("t-more").is_none(), "nothing folded away");
    d.click_key(&mut app, tool);
    d.frame(&mut app);
    assert!(blocks(&app).iter().any(|b| matches!(b.kind, Kind::Image { float: Float::Right, .. })));
    let tool = d.key_of("t-inline").unwrap();
    d.click_key(&mut app, tool);
    d.frame(&mut app);
    assert!(blocks(&app).iter().any(|b| matches!(b.kind, Kind::Image { float: Float::None, .. })));
    let _ = std::fs::remove_dir_all(dir);
}

/// The toolbar shows the tools for what the caret is in, and only those:
/// walked through a paragraph, a list's items, a table's cell, code and a
/// picture, each set has its own, and another's are not there at all —
/// not greyed. What applies but can't act now (a row's move up from under
/// the header) is greyed. Arrowing along the table changes nothing.
#[test]
fn the_toolbar_shows_the_tools_for_what_the_caret_is_in() {
    // Wide, so every set shows whole and nothing is in More.
    let (store, dir) = temp_store("tool-sets");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1800.0, 760.0).framing();
    d.frame(&mut app);
    app.new_note();
    d.frame(&mut app);
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    let table = Table::from_cells(vec![
        vec!["Day".into(), "Miles".into()],
        vec!["Mon".into(), "12".into()],
        vec!["Tue".into(), "30".into()],
    ]);
    let (mon, day) = (table.rows[1].id, table.cols[0]);
    let picture = Kind::Image { src: "nowhere.png".into(), alt: String::new(), width: None, float: Float::None, wiki: false };
    app.store.notes[app.current].doc = Doc::new(vec![
        Block::text("Words"),
        Block::new(Kind::item(ListStyle::Bullet, 0, "one")),
        Block::new(Kind::item(ListStyle::Todo, 0, "two")),
        Block::new(Kind::Table(table)),
        Block::new(Kind::Raw("```\nlet x = 1;\n```".into())),
        Block::new(picture),
        Block::text(""),
    ]);
    let ids: Vec<u64> = blocks(&app).iter().map(|b| b.id).collect();
    d.core.set_inspect(true);
    let enabled = |d: &mut Drive, key: &str| {
        let k = d.key_of(key).expect(key);
        d.core.nodes().iter().any(|n| n.key == k && !n.flags.contains(&"disabled"))
    };
    // The caret to `c`, and the strip drawn for it.
    let go = |app: &mut Carnet, d: &mut Drive, c: Caret| {
        app.caret = c;
        d.frame(app);
        d.frame(app);
    };
    let check = |d: &mut Drive, app: &Carnet, set: chrome::ToolSet, shown: &[&str], absent: &[&str]| {
        assert_eq!(app.tool_set, Some(set));
        assert!(app.overflow.is_empty(), "{set:?}: nothing folded into More");
        for k in shown {
            assert!(d.key_of(k).is_some(), "{set:?} shows {k}");
        }
        for k in absent {
            assert!(d.key_of(k).is_none(), "{set:?} has no {k}, greyed or not");
        }
    };

    // A paragraph: what it becomes, its styles, what to put in, its moves.
    go(&mut app, &mut d, Caret::at(Pos { slot: Slot::block(ids[0]), byte: 2 }));
    check(
        &mut d,
        &app,
        chrome::ToolSet::Words,
        &["t-text", "t-h1", "t-h2", "t-h3", "t-bullet", "t-todo", "t-quote", "t-initial", "t-bold", "t-code", "t-mark", "t-color", "t-table", "t-rule", "t-attach", "t-up", "t-down", "t-del"],
        &["t-out", "t-in", "t-tick", "t-addrow", "t-rowup", "t-sort", "t-inline", "t-left"],
    );
    assert!(enabled(&mut d, "t-initial"), "a paragraph's first letter drops");

    // A bullet: a list's kinds and back to text, in and out; no headings.
    go(&mut app, &mut d, Caret::at(Pos { slot: Slot::block(ids[1]), byte: 1 }));
    check(
        &mut d,
        &app,
        chrome::ToolSet::List,
        &["t-text", "t-bullet", "t-number", "t-todo", "t-out", "t-in", "t-bold", "t-color", "t-table", "t-attach", "t-up", "t-del"],
        &["t-h1", "t-h2", "t-h3", "t-quote", "t-initial", "t-tick", "t-addrow", "t-inline"],
    );
    // A to-do: its tick too.
    go(&mut app, &mut d, Caret::at(Pos { slot: Slot::block(ids[2]), byte: 1 }));
    check(&mut d, &app, chrome::ToolSet::List, &["t-todo", "t-out", "t-in", "t-tick"], &["t-h1", "t-initial"]);
    let tick = d.key_of("t-tick").unwrap();
    d.click_key(&mut app, tick);
    d.frame(&mut app);
    assert!(matches!(blocks(&app)[2].kind, Kind::Item { done: true, .. }), "ticked from the toolbar");

    // A table's cell: its rows and columns, the styles, a picture; nothing
    // a block becomes, nor a table or a divider to put in it.
    let cell = Slot { block: ids[3], cell: Some((mon, day)) };
    go(&mut app, &mut d, Caret::at(Pos { slot: cell, byte: 1 }));
    check(
        &mut d,
        &app,
        chrome::ToolSet::Table,
        &["t-addrow", "t-addcol", "t-delrow", "t-delcol", "t-rowup", "t-rowdown", "t-colleft", "t-colright", "t-header", "t-sort", "t-del", "t-bold", "t-color", "t-attach"],
        &["t-text", "t-h1", "t-bullet", "t-todo", "t-quote", "t-initial", "t-out", "t-in", "t-table", "t-rule", "t-up", "t-down", "t-inline"],
    );
    assert!(!enabled(&mut d, "t-rowup") && enabled(&mut d, "t-rowdown"), "greyed under the header, not gone");
    // Along the row and down the column: the same set, nothing moving.
    let since = app.tool_set_at;
    key(&mut app, &mut d, "tab");
    key(&mut app, &mut d, "down");
    assert!(app.caret.head.slot.cell.is_some(), "still in the table");
    assert_eq!((app.tool_set, app.tool_set_at), (Some(chrome::ToolSet::Table), since), "no change, no motion");

    // Code: its moves and what to put in after it; no styles.
    go(&mut app, &mut d, Caret::at(Pos { slot: Slot::block(ids[4]), byte: 5 }));
    check(
        &mut d,
        &app,
        chrome::ToolSet::Code,
        &["t-up", "t-down", "t-del", "t-table", "t-rule", "t-attach"],
        &["t-text", "t-h1", "t-bullet", "t-initial", "t-bold", "t-code", "t-mark", "t-color", "t-out", "t-addrow", "t-inline"],
    );

    // A picture: where it sits, as it always had.
    let chosen = page::select_blocks(app.doc(), 5, 5);
    go(&mut app, &mut d, chosen);
    check(
        &mut d,
        &app,
        chrome::ToolSet::Picture,
        &["t-inline", "t-left", "t-right", "t-up", "t-down", "t-del", "t-table", "t-attach"],
        &["t-text", "t-h1", "t-bold", "t-color", "t-out", "t-addrow", "t-initial"],
    );

    // The title: only what goes in at the page's caret.
    app.focus(Field::Title);
    d.frame(&mut app);
    d.frame(&mut app);
    check(&mut d, &app, chrome::ToolSet::Away, &["t-table", "t-rule", "t-attach", "t-pin", "t-trash"], &["t-text", "t-bold", "t-up", "t-addrow", "t-inline"]);

    // Narrow, in the table: what doesn't fit is in More, and it is the
    // table's set there too.
    let mut narrow = Drive::new(Core::new(), 620.0, 600.0).framing();
    narrow.core.set_inspect(true);
    app.edit_at(Pos { slot: cell, byte: 1 });
    for _ in 0..3 {
        narrow.frame(&mut app);
    }
    assert_eq!(app.tool_set, Some(chrome::ToolSet::Table));
    assert!(narrow.key_of("t-more").is_some() && narrow.key_of("t-addrow").is_some(), "the table's first tools, then More");
    let menu: Vec<String> = app.tools_menu().iter().map(|m| m.label.clone()).filter(|l| !l.is_empty()).collect();
    assert!(menu.iter().any(|l| l == "Add pictures or files"), "{menu:?}");
    assert!(!menu.iter().any(|l| l == "Heading 1" || l == "Move up" || l == "Divider"), "{menu:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn notes_sort_by_when_they_were_made() {
    let (mut app, mut d, dir) = fresh("sort-created");
    let base = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    for (i, t) in ["Old", "Newest", "Middle"].iter().enumerate() {
        app.new_note();
        d.frame(&mut app);
        let n = &mut app.store.notes[app.current];
        n.title = t.to_string();
        n.doc = Doc::new(vec![Block::text("x")]);
        n.created = base + Duration::from_secs([1, 30, 20][i]);
        // Edited the other way round.
        n.modified = base + Duration::from_secs([30, 1, 20][i]);
    }
    app.folder_action("", "sort-created");
    let titles: Vec<&str> = app.order.iter().map(|&i| app.store.notes[i].title.as_str()).filter(|t| ["Old", "Newest", "Middle"].contains(t)).collect();
    assert_eq!(titles, ["Newest", "Middle", "Old"]);
    assert!(crate::settings::Settings::parse("sort = created").sort == crate::settings::Sort::Created);
    let _ = std::fs::remove_dir_all(dir);
}

/// The picker's swatch for a hue, by its name.
fn click_hue(app: &mut Carnet, d: &mut Drive, name: &str) {
    let k = d.key_named(name).expect(name);
    d.click_key(app, k);
    d.frame(app);
    d.frame(app);
}

#[test]
fn a_colour_and_a_highlighter_on_the_same_words_from_the_picker() {
    let (mut app, mut d, dir) = body("tint-both");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "some red words");
    let slot = app.caret.head.slot;
    // Double-click "red".
    let k = *app.slot_keys.get(&slot).unwrap();
    let r = d.core.caret_rect(k, 6).unwrap();
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(r.x + 1.0, r.y + r.h / 2.0)));
    d.input(&mut app, InputEvent::MouseDown { button: kui_native::MouseButton::Primary, clicks: 2 });
    d.input(&mut app, InputEvent::MouseUp { button: kui_native::MouseButton::Primary });
    d.frame(&mut app);
    let tool = d.key_of("t-color").unwrap();
    d.click_key(&mut app, tool);
    d.frame(&mut app);
    d.frame(&mut app);
    click_hue(&mut app, &mut d, "Red");
    // The picker stays: the other tab gives the highlighter.
    assert!(app.tint_open.is_some(), "the picker stays open for the other colour");
    let tab = d.key_of("tab-mark").expect("the highlighter's tab");
    d.click_key(&mut app, tab);
    d.frame(&mut app);
    d.frame(&mut app);
    click_hue(&mut app, &mut d, "Yellow");
    assert_eq!(texts(&app)[0], "some <span style=\"color: #C0553F; background: #F8EBA6CC\">red</span> words");
    // From the toolbar again, the other way round, on another word.
    key(&mut app, &mut d, "escape");
    let r = d.core.caret_rect(*app.slot_keys.get(&slot).unwrap(), 11).unwrap();
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(r.x + 1.0, r.y + r.h / 2.0)));
    d.input(&mut app, InputEvent::MouseDown { button: kui_native::MouseButton::Primary, clicks: 2 });
    d.input(&mut app, InputEvent::MouseUp { button: kui_native::MouseButton::Primary });
    d.frame(&mut app);
    let tool = d.key_of("t-mark").unwrap();
    d.click_key(&mut app, tool);
    d.frame(&mut app);
    d.frame(&mut app);
    click_hue(&mut app, &mut d, "Yellow");
    // While it is open, the other colour is its other tab.
    let tab = d.key_of("tab-ink").unwrap();
    d.click_key(&mut app, tab);
    d.frame(&mut app);
    d.frame(&mut app);
    click_hue(&mut app, &mut d, "Blue");
    assert!(
        texts(&app)[0].ends_with(" <span style=\"color: #5279B3; background: #F8EBA6CC\">words</span>"),
        "{}",
        texts(&app)[0]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_picture_is_carried_between_lines_and_beside_another() {
    let (mut app, mut d, dir) = with_picture("pic-dnd");
    // Words under the first picture, then a second picture.
    let tail = blocks(&app).last().unwrap().id;
    app.caret = Caret::at(page::start(Slot::block(tail)));
    type_in(&mut app, &mut d, "Below");
    key(&mut app, &mut d, "enter");
    let rgba: Vec<u8> = (0..300 * 200).flat_map(|_| [90u8, 120, 200, 255]).collect();
    app.paste_picture(300, 200, &rgba);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let pics: Vec<u64> = blocks(&app).iter().filter(|b| matches!(b.kind, Kind::Image { .. })).map(|b| b.id).collect();
    assert_eq!(pics.len(), 2, "{:?}", texts(&app));
    let rect = |app: &Carnet, id: u64| app.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r).unwrap();
    let above = blocks(&app)[0].id;

    // Pressed inside a picture, clear of its handles.
    let grip = |r: Rect| Vec2::new(r.x + 60.0, r.y + r.h / 2.0);
    // The second picture carried above the first line.
    let from = grip(rect(&app, pics[1]));
    let to = rect(&app, above);
    d.drag(&mut app, from, Vec2::new(to.x + 40.0, to.y + 2.0));
    d.frame(&mut app);
    assert_eq!(blocks(&app)[0].id, pics[1], "{:?}", texts(&app));
    assert!(app.pic_drag.is_none() && app.drop_index.is_none());
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(blocks(&app)[0].id, above, "one undo puts it back");
    for _ in 0..3 {
        d.frame(&mut app);
    }

    // Let go over the first picture's right side: the two side by side.
    let from = grip(rect(&app, pics[1]));
    let r1 = rect(&app, pics[0]);
    d.drag(&mut app, from, Vec2::new(r1.x + r1.w * 0.9, r1.y + r1.h / 2.0));
    for _ in 0..6 {
        d.frame(&mut app);
    }
    let ids: Vec<u64> = blocks(&app).iter().map(|b| b.id).collect();
    let i = ids.iter().position(|&b| b == pics[0]).unwrap();
    assert_eq!(ids[i + 1], pics[1], "after it");
    for id in &pics {
        assert!(matches!(app.doc().get(*id).unwrap().kind, Kind::Image { float: Float::Left, .. }));
    }
    // Drawn side by side, apart, on one row.
    let (a, b) = (rect(&app, pics[0]), rect(&app, pics[1]));
    assert!((a.y - b.y).abs() < 2.0, "{a:?} {b:?}");
    assert!(a.x + a.w <= b.x + 1.0, "no overlap: {a:?} {b:?}");
    let imgs: Vec<Rect> = d.core.nodes().iter().filter(|n| n.role == Some(Role::Image)).map(|n| n.rect).collect();
    let mut row: Vec<Rect> = imgs.into_iter().filter(|r| (r.y - a.y).abs() < 40.0).collect();
    row.sort_by(|p, q| p.x.total_cmp(&q.x));
    row.dedup_by(|p, q| (p.x - q.x).abs() < 1.0);
    assert!(row.len() >= 2 && row[0].x + row[0].w + 10.0 <= row[1].x, "the pictures themselves apart: {row:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Two pictures at the left edge, one on a line of its own under them: a
/// divider put in after the second goes between, with no empty line made
/// under it, and is drawn across the page under the pictures — not in a
/// column beside them, where the pictures' height showed as a gap.
#[test]
fn a_divider_after_pictures_at_the_edge_has_no_gap_under_it() {
    let (mut app, mut d, dir) = body("rule-floats");
    for shade in [200u8, 90, 120] {
        let rgba: Vec<u8> = (0..300 * 200).flat_map(|_| [shade, 120, 150, 255]).collect();
        app.paste_picture(300, 200, &rgba);
        d.frame(&mut app);
    }
    let pics: Vec<u64> = blocks(&app).iter().filter(|b| matches!(b.kind, Kind::Image { .. })).map(|b| b.id).collect();
    assert_eq!(pics.len(), 3, "{:?}", texts(&app));
    for &id in &pics[..2] {
        app.edit(Step::Other, |doc, _| page::set_image_float(doc, id, Float::Left) && page::set_image_width(doc, id, Some(160)));
    }
    let i = app.doc().index_of(pics[1]).unwrap();
    app.caret = page::select_blocks(app.doc(), i, i);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    app.on_plain(Msg::InsertDivider);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let kinds: Vec<String> = blocks(&app)
        .iter()
        .map(|b| match &b.kind {
            Kind::Image { .. } => "picture".to_string(),
            Kind::Divider => "---".to_string(),
            _ => format!("text:{}", b.text_ref().unwrap_or("")),
        })
        .collect();
    let at = kinds.iter().position(|k| k == "---").expect("a divider");
    assert_eq!(kinds[at - 2..at + 2], ["picture", "picture", "---", "picture"], "{kinds:?}");
    let rect = |id: u64| app.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r).unwrap();
    let (first, rule, below) = (rect(pics[0]), rect(blocks(&app)[at].id), rect(pics[2]));
    assert!(rule.y >= first.y + first.h - 1.0, "under the pictures: {first:?} {rule:?}");
    assert!((rule.x - first.x).abs() < 2.0, "across the page, from its left: {first:?} {rule:?}");
    assert!(below.y - (rule.y + rule.h) < 40.0, "no gap under it: {rule:?} {below:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The note the three renderings were compared on: a picture at the left
/// edge, a heading and a checklist beside it, then a table. The words run
/// beside the picture; the table starts below its foot, as wide as the
/// text, its columns sharing the width — not squeezed in beside it, its
/// cells wrapping.
#[test]
fn a_table_after_a_picture_at_the_edge_starts_below_it_at_full_width() {
    let (store, dir) = temp_store("table-below-float");
    drop(store);
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    std::fs::create_dir_all(dir.join("Trips")).unwrap();
    let rgba: Vec<u8> = (0..356 * 356).flat_map(|_| [120u8, 150, 170, 255]).collect();
    std::fs::write(dir.join("attachments/coast.png"), crate::images::encode_png(356, 356, &rgba).unwrap()).unwrap();
    let md = "# Big Sur\n\n<span class=\"initial\">T</span>he coast road runs for ninety miles between the mountains and the sea.\n\n\
              ![|left|330](../attachments/coast.png)\n\n## Plan\n\n- [x] Book the cabin in Big Sur\n- [x] Fill the tank in Carmel\n\
              - [ ] Pfeiffer Beach at sunset\n- [ ] McWay Falls\n\n| Day | Where | Miles |\n|---|---|---|\n| Fri | Carmel → Big Sur | 30 |\n\
              | Sat | Big Sur → Ragged Point | 45 |\n| Sun | Ragged Point → Cambria | 25 |\n\n---\n\n> Bring a jacket.\n";
    std::fs::write(dir.join("Trips/Big Sur.md"), md).unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1280.0, 900.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    let i = app.store.notes.iter().position(|n| n.title == "Big Sur").unwrap();
    app.open(i);
    for _ in 0..8 {
        d.frame(&mut app);
    }
    let rect = |kind: fn(&Kind) -> bool| {
        let id = blocks(&app).iter().find(|b| kind(&b.kind)).unwrap().id;
        app.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r).unwrap()
    };
    let words = rect(|k| matches!(k, Kind::Text(_)));
    let pic = rect(|k| matches!(k, Kind::Image { .. }));
    let heading = rect(|k| matches!(k, Kind::Heading { .. }));
    let table = rect(|k| matches!(k, Kind::Table(_)));
    let img = d.core.nodes().iter().find(|n| n.role == Some(Role::Image) && n.rect.w > 100.0 && n.rect.w < 400.0).map(|n| n.rect).unwrap();
    assert!((img.w - 330.0).abs() < 0.5, "the width the note gives it: {img:?}");
    // The heading beside the picture; the table below its foot.
    assert!(heading.x > pic.x + pic.w - 1.0 && heading.y < pic.y + pic.h, "beside: {pic:?} {heading:?}");
    assert!(table.y >= pic.y + pic.h - 1.0, "below the picture: {pic:?} {table:?}");
    assert!((table.x - words.x).abs() < 1.0 && (table.w - words.w).abs() < 1.0, "the text's width: {words:?} {table:?}");
    // Its columns share it alike, a third each.
    let cell = |s: &str| d.core.nodes().iter().find(|n| n.text.as_deref() == Some(s)).map(|n| n.rect).unwrap();
    let (day, place, miles) = (cell("Day"), cell("Where"), cell("Miles"));
    let third = (table.w - 16.0) / 3.0;
    assert!(((place.x - day.x) - third).abs() < 3.0 && ((miles.x - place.x) - third).abs() < 3.0, "{table:?} {day:?} {place:?} {miles:?}");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Lets what eases get where it is going: the frame clock moved on.
fn settle(app: &mut Carnet, d: &mut Drive) {
    d.advance(1.0);
    d.frame(app);
}

/// A tool's opacity as drawn (its node, by key label).
fn tool_opacity(d: &Drive, key: &str) -> f32 {
    d.core.nodes().iter().find(|n| n.label.as_deref() == Some(key)).map(|n| n.opacity).expect(key)
}

#[test]
fn typing_puts_the_tools_away_until_they_are_wanted() {
    let (mut app, mut d, dir) = body("flow");
    d.core.set_inspect(true);
    settle(&mut app, &mut d);
    assert!(!app.in_flow(), "not before a word is typed");
    assert_eq!(tool_opacity(&d, "t-bold"), 1.0);
    type_in(&mut app, &mut d, "Words going down");
    assert!(app.in_flow());
    settle(&mut app, &mut d);
    assert_eq!(tool_opacity(&d, "t-bold"), 0.0, "the tools step out of the way");
    assert_eq!(tool_opacity(&d, "t-trash"), 0.0);
    // Enter and Backspace are writing too; the arrows change nothing.
    key(&mut app, &mut d, "enter");
    key(&mut app, &mut d, "backspace");
    key(&mut app, &mut d, "left");
    assert!(app.in_flow());
    // A hand going to the pointer brings them back; a nudge does not.
    d.move_to(&mut app, 700.0, 300.0);
    d.frame(&mut app);
    type_in(&mut app, &mut d, " more");
    d.move_to(&mut app, 703.0, 302.0);
    d.frame(&mut app);
    assert!(app.in_flow(), "a nudge leaves them away");
    d.move_to(&mut app, 700.0, 20.0);
    settle(&mut app, &mut d);
    assert!(!app.in_flow());
    assert_eq!(tool_opacity(&d, "t-bold"), 1.0);
    // So does a selection: there is something to style.
    type_in(&mut app, &mut d, "!");
    assert!(app.in_flow());
    key_with(&mut app, &mut d, "left", SHIFT);
    assert!(!app.in_flow());
    // A chord while writing — the line taken back, a move to its start —
    // is still writing.
    type_in(&mut app, &mut d, " and more");
    key_with(&mut app, &mut d, "backspace", CTRL);
    key_with(&mut app, &mut d, "home", CTRL);
    assert!(app.in_flow());
    // And Esc.
    type_in(&mut app, &mut d, "?");
    key(&mut app, &mut d, "escape");
    assert!(!app.in_flow());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_toolbar_stays_when_the_settings_say_so() {
    let (mut app, mut d, dir) = body("stays");
    app.settings.fade_tools = false;
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "Words");
    assert!(app.in_flow());
    settle(&mut app, &mut d);
    assert_eq!(tool_opacity(&d, "t-bold"), 1.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn focus_steps_the_other_blocks_back_while_typing() {
    let (mut app, mut d, dir) = body("focus");
    app.settings.focus = true;
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "First");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "Second");
    let ids: Vec<u64> = blocks(&app).iter().map(|b| b.id).collect();
    // Each block's row, found by where it was laid out.
    let opacity = |d: &Drive, app: &Carnet, id: u64| {
        let r = app.row_rects.iter().find(|(i, _)| *i == id).map(|(_, r)| *r).unwrap();
        d.core
            .nodes()
            .iter()
            .find(|n| n.rect == r && n.events.iter().any(|(k, _)| *k == "layout"))
            .map(|n| n.opacity)
            .unwrap()
    };
    settle(&mut app, &mut d);
    assert!(opacity(&d, &app, ids[0]) < 0.5, "the block written before steps back");
    assert_eq!(opacity(&d, &app, ids[1]), 1.0, "the one being written does not");
    // The pointer up to the strip: everything is back.
    d.move_to(&mut app, 700.0, 20.0);
    settle(&mut app, &mut d);
    assert_eq!(opacity(&d, &app, ids[0]), 1.0);
    let _ = std::fs::remove_dir_all(dir);
}

/// Letters typed on the keyboard come as key presses carrying their text,
/// not as an input method's text: they are writing as much as Enter is.
/// The toolbar fades, and with Focus on the other blocks step back.
#[test]
fn letters_typed_from_the_keyboard_are_writing_too() {
    let (mut app, mut d, dir) = body("letters");
    app.settings.focus = true;
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "First");
    key(&mut app, &mut d, "enter");
    // Out of the writing: the pointer up to the strip.
    d.move_to(&mut app, 700.0, 20.0);
    settle(&mut app, &mut d);
    assert!(!app.in_flow());
    assert_eq!(tool_opacity(&d, "t-bold"), 1.0);
    for c in ["h", "i"] {
        key(&mut app, &mut d, c);
    }
    assert_eq!(texts(&app)[..2], ["First", "hi"]);
    assert!(app.in_flow(), "typed letters are typing");
    settle(&mut app, &mut d);
    assert_eq!(tool_opacity(&d, "t-bold"), 0.0, "the toolbar fades");
    let first = blocks(&app)[0].id;
    let r = app.row_rects.iter().find(|(i, _)| *i == first).map(|(_, r)| *r).unwrap();
    let dim = d.core.nodes().iter().find(|n| n.rect == r && n.events.iter().any(|(k, _)| *k == "layout")).map(|n| n.opacity).unwrap();
    assert!(dim < 0.5, "the block written before steps back: {dim}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_empty_line_waits_for_a_pause_before_it_hints() {
    let (mut app, mut d, dir) = body("pause");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "First");
    key(&mut app, &mut d, "enter");
    let hinted = |d: &Drive| d.core.nodes().iter().any(|n| n.text.as_deref() == Some("Type / for blocks"));
    assert!(!hinted(&d), "not on the line Enter just made");
    d.advance(1.5);
    d.frame(&mut app);
    assert!(hinted(&d), "once the writing pauses");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn writing_keeps_room_under_the_line_being_written() {
    let (store, dir) = temp_store("room");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 520.0).framing();
    d.frame(&mut app);
    app.new_note();
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    // At a person's pace: what the page glides to gets there between keys.
    for i in 0..24 {
        type_in(&mut app, &mut d, &format!("Line {i}"));
        key(&mut app, &mut d, "enter");
        d.advance(0.3);
        d.frame(&mut app);
    }
    type_in(&mut app, &mut d, "The last");
    for _ in 0..4 {
        d.advance(0.5);
        d.frame(&mut app);
    }
    let page = app.page_rect.unwrap();
    let caret = app.rect_at(&d.core, app.caret.head.slot, app.caret.head.byte).unwrap();
    let room = page.y + page.h - (caret.y + caret.h);
    eprintln!("page {page:?} caret {caret:?} geo {:?}", d.core.scroll_geometry(app.scroll_key.unwrap()));
    assert!(room >= 48.0, "{room}px under the caret");
    assert!(caret.y >= page.y, "and the caret in sight");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_colour_and_a_highlight_go_on_together_from_the_toolbar() {
    let (mut app, mut d, dir) = body("both");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "the open questions");
    key_with(&mut app, &mut d, "home", SHIFT);
    let click_tool = |app: &mut Carnet, d: &mut Drive, key: &str| {
        let r = d.core.nodes().iter().find(|n| n.label.as_deref() == Some(key)).map(|n| n.rect).expect(key);
        d.click(app, r.x + r.w / 2.0, r.y + r.h / 2.0);
        d.frame(app);
        d.frame(app);
    };
    click_tool(&mut app, &mut d, "t-color");
    let k = d.key_named("Red").expect("a colour");
    d.click_key(&mut app, k);
    d.frame(&mut app);
    // The picker stays; the highlighter's button switches it over.
    click_tool(&mut app, &mut d, "t-mark");
    assert_eq!(app.tint_open.map(|t| t.0), Some(false), "on the highlighter now");
    let k = d.key_named("Green").expect("a highlighter");
    d.click_key(&mut app, k);
    d.frame(&mut app);
    assert_eq!(texts(&app), ["<span style=\"color: #C0553F; background: #D8EAC0CC\">the open questions</span>"]);
    // Writing on puts it away.
    key(&mut app, &mut d, "end");
    type_in(&mut app, &mut d, ".");
    assert!(app.tint_open.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_library_hides_and_comes_back() {
    let (mut app, mut d, dir) = body("sidebar");
    d.core.set_inspect(true);
    let cards = |d: &mut Drive| d.key_of("library").is_some();
    settle(&mut app, &mut d);
    assert!(cards(&mut d));
    // Mod+\ from the page: the page has the window, once the pane has slid
    // shut.
    key_with(&mut app, &mut d, "\\", CTRL);
    assert!(!app.settings.sidebar);
    assert_eq!(app.side_w(false), 0.0);
    settle(&mut app, &mut d);
    settle(&mut app, &mut d);
    assert!(!cards(&mut d), "nothing of the library is left");
    assert!(d.key_of("t-sidebar").is_some(), "the strip has the way back");
    // Searching wants the library: it comes back.
    key_with(&mut app, &mut d, "f", CTRL);
    settle(&mut app, &mut d);
    assert!(app.settings.sidebar && cards(&mut d));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tab_goes_search_notes_title_note() {
    let (mut app, mut d, dir) = body("tabs");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "Words");
    // Esc twice: out of the page, which keeps Tab for indenting.
    key(&mut app, &mut d, "escape");
    key(&mut app, &mut d, "escape");
    let mut seen = Vec::new();
    for _ in 0..5 {
        key(&mut app, &mut d, "tab");
        let f = d.core.focus();
        let n = d.core.nodes().iter().find(|n| Some(n.key) == f).cloned().unwrap();
        seen.push(n.label.unwrap_or_else(|| format!("{:?}", n.role.unwrap())));
    }
    // The toolbar, the library's buttons and the settings are the
    // pointer's and the shortcuts': none of them is on the way.
    assert_eq!(seen, ["search", "Button", "Button", "title", "body"], "two notes: the welcome note and this one");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zen_leaves_the_note_alone_on_the_screen() {
    let (mut app, mut d, dir) = body("zen");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "Words");
    d.move_to(&mut app, 700.0, 400.0);
    key_with(&mut app, &mut d, ".", CTRL);
    assert!(app.zen && app.focus_on() && app.typewriter_on());
    assert_eq!(app.side_w(false), 0.0, "the library goes");
    assert!(app.settings.sidebar && !app.settings.focus, "and the settings are as they were");
    settle(&mut app, &mut d);
    assert_eq!(tool_opacity(&d, "t-bold"), 0.0, "the tools are away, typing or not");
    // Up to the strip: they are there.
    d.move_to(&mut app, 700.0, 20.0);
    settle(&mut app, &mut d);
    assert_eq!(tool_opacity(&d, "t-bold"), 1.0);
    // Mod+. again: all back.
    key_with(&mut app, &mut d, ".", CTRL);
    assert!(!app.zen && app.side_w(false) > 0.0);
    // The library's own switch also ends it.
    key_with(&mut app, &mut d, ".", CTRL);
    app.on_plain(Msg::ToggleSidebar);
    assert!(!app.zen && app.settings.sidebar);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_deleted_folder_puts_anything_back_where_it_was() {
    let (mut app, mut d, dir) = fresh("deleted");
    d.text(&mut app, "Kept for later");
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "Words");
    // A note in a folder, and the folder.
    let folder = app.store.create_folder("", "Work").unwrap();
    app.move_note(app.current, &folder);
    app.delete_note_at(app.current);
    assert!(app.trash.iter().any(|t| t.title == "Kept for later" && t.folder == "Work"));
    app.trash_folder("Work");
    d.frame(&mut app);
    // Long after the notice: the Deleted folder still has them.
    app.notice = None;
    app.on_plain(Msg::ShowDeleted);
    d.frame(&mut app);
    assert!(app.deleted);
    let titles: Vec<&str> = app.trash.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Work", "Kept for later"], "the latest gone first");
    // The folder back, then the note into it.
    app.on_plain(Msg::RestoreTrashed { index: 0 });
    assert!(app.store.folders.iter().any(|f| f == "Work"));
    app.on_plain(Msg::ShowDeleted);
    app.on_plain(Msg::RestoreTrashed { index: 0 });
    let n = &app.store.notes[app.current];
    assert_eq!((n.display_title(), n.folder.as_str()), ("Kept for later", "Work"), "back where it was, and open");
    assert!(dir.join("Work").join("Kept for later.md").exists());
    assert!(app.trash.is_empty() && !app.deleted, "nothing left to show");
    let _ = std::fs::remove_dir_all(dir);
}

/// A deleted note opens to be read, not written: typing changes nothing.
/// From there it's restored, or deleted forever once asked to be sure;
/// a deleted folder opens as its notes' titles, and goes the same way.
#[test]
fn a_deleted_note_opens_to_read_then_comes_back_or_goes_forever() {
    let (mut app, mut d, dir) = fresh("forever");
    d.text(&mut app, "Old plans");
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "Never again");
    app.delete_note_at(app.current);
    app.store.create_folder("", "Archive").unwrap();
    std::fs::write(dir.join("Archive").join("Inside.md"), "# Inside\n\nWords\n").unwrap();
    app.store.rescan();
    app.trash_folder("Archive");
    app.notice = None;
    app.on_plain(Msg::ShowDeleted);
    d.frame(&mut app);
    let i = app.trash.iter().position(|t| t.title == "Old plans").unwrap();

    // Opened: its words on the page, read only.
    app.on_plain(Msg::OpenDeleted { index: i as u64 });
    d.frame(&mut app);
    assert!(app.reading() && carnet_core::doc::to_markdown(None, app.doc()).contains("Never again"));
    let before = carnet_core::doc::to_markdown(None, app.doc());
    type_in(&mut app, &mut d, "more");
    assert_eq!(carnet_core::doc::to_markdown(None, app.doc()), before, "read, not written");
    // The note behind it isn't pinned or trashed from here.
    let behind = app.store.notes.len();
    app.on_plain(Msg::DeleteNote);
    app.on_plain(Msg::PinNote);
    assert!(app.store.notes.len() == behind && !app.store.notes.iter().any(|n| n.pinned));

    // Delete forever asks first; Cancel keeps it.
    app.on_plain(Msg::DeleteForever);
    assert!(app.peek.as_ref().unwrap().armed);
    app.on_plain(Msg::DeleteForeverCancel);
    assert!(!app.peek.as_ref().unwrap().armed && app.trash.len() == 2);
    // Asked and sure: gone, file and all.
    app.on_plain(Msg::DeleteForever);
    app.on_plain(Msg::DeleteForever);
    d.frame(&mut app);
    assert!(app.peek.is_none() && !app.reading());
    assert!(!app.trash.iter().any(|t| t.title == "Old plans"));
    assert!(std::fs::read_dir(dir.join(".trash")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with("Old plans")));

    // The folder: its notes' titles; restored from there, it's back.
    app.on_plain(Msg::OpenDeleted { index: 0 });
    d.frame(&mut app);
    assert!(carnet_core::doc::to_markdown(None, app.doc()).contains("Inside"));
    app.on_plain(Msg::RestorePeek);
    assert!(app.peek.is_none() && app.store.folders.iter().any(|f| f == "Archive"));
    assert!(app.trash.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_title_and_the_note_are_one_text_to_the_arrows() {
    let (mut app, mut d, dir) = fresh("title-arrows");
    d.text(&mut app, "Plans");
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "First line");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "Second");
    // Up, up: from the second line to the first, then into the title,
    // under the column the caret was in, as one text would.
    key(&mut app, &mut d, "up");
    assert!(page_focused(&app, &d));
    let column = app.rect_at(&d.core, app.caret.head.slot, app.caret.head.byte).unwrap().x;
    key(&mut app, &mut d, "up");
    d.frame(&mut app);
    let title = d.core.focus().and_then(|k| app.fields.get(&k)).copied();
    assert_eq!(title, Some(Field::Title), "up from the first line is the title");
    let k = d.core.focus().unwrap();
    let byte = d.core.edit.selection_cursors(k).map(|s| s.1 .1).unwrap();
    assert!(byte > 0 && byte < 5, "inside \"Plans\", not at its end: {byte}");
    let at = d.core.edit_caret_rect(k).unwrap().x;
    assert!((at - column).abs() < 12.0, "under the column: {at} vs {column}");
    // Enter or Tab: back into the note (and ↓, below).
    key(&mut app, &mut d, "tab");
    d.frame(&mut app);
    assert!(page_focused(&app, &d), "Tab from the title is the note");
    // Back from the very start is the title too.
    key_with(&mut app, &mut d, "up", CTRL);
    assert_eq!(app.caret.head, page::first(app.doc()));
    key(&mut app, &mut d, "left");
    d.frame(&mut app);
    assert_eq!(d.core.focus().and_then(|k| app.fields.get(&k)).copied(), Some(Field::Title));
    let _ = std::fs::remove_dir_all(dir);
}

/// A row of the Mac's menu bar chosen, as AppKit reports it: by the menu's
/// title and the row's label.
fn choose(app: &mut Carnet, d: &mut Drive, menu: &str, row: &str) {
    let bar = d.core.menu_bar().cloned().expect("a declared bar");
    let mi = bar.menus.iter().position(|m| m.label == menu).expect(menu);
    let ii = bar.menus[mi].items.iter().position(|i| i.text() == row).expect(row);
    for ev in d.core.activate_menu_bar_item(mi, ii) {
        app.on_event_with(ev, &mut d.core);
    }
    d.frame(app);
}

/// A fresh note, as on a Mac: the menu bar declared.
fn fresh_mac(name: &str) -> (Carnet, Drive, std::path::PathBuf) {
    let (mut app, mut d, dir) = fresh(name);
    d.core.set_native_menu_bar(true);
    d.frame(&mut app);
    (app, d, dir)
}

#[test]
fn the_menu_bar_plays_its_keys_where_the_keyboard_is() {
    let (mut app, mut d, dir) = fresh_mac("menubar");
    // Every row with a shortcut is its key, the same wherever the
    // keyboard is; only the app's own commands post.
    let bar = app.menu_bar(&d.core);
    for item in bar.menus.iter().flat_map(|m| &m.items).filter(|i| i.accel.is_some()) {
        let own = matches!(item.text(), "Close Window" | "Minimize");
        assert_eq!(item.replay, !own, "{}", item.text());
    }
    // In the title: Undo is the field's.
    d.text(&mut app, "Plans");
    d.frame(&mut app);
    let title = d.core.focus().unwrap();
    assert_eq!(app.fields.get(&title), Some(&Field::Title));
    choose(&mut app, &mut d, "Edit", "Undo");
    assert_eq!(d.core.edit_text(title).as_deref(), Some(""), "the title's typing undone");
    assert_eq!(d.core.focus(), Some(title));
    // A chord the field has no use for goes on to the app's: Zen Mode,
    // which leaves the keyboard in the note.
    choose(&mut app, &mut d, "View", "Zen Mode");
    assert!(app.zen);
    assert!(page_focused(&app, &d));
    choose(&mut app, &mut d, "View", "Zen Mode");
    assert!(!app.zen);
    // In the note: Format ▸ Bold is ⌘B, played to the page.
    type_in(&mut app, &mut d, "loud");
    assert_eq!(texts(&app)[0], "loud");
    key_with(&mut app, &mut d, "home", SHIFT);
    choose(&mut app, &mut d, "Format", "Bold");
    assert_eq!(texts(&app)[0], "**loud**");
    assert!(page_focused(&app, &d));
    // Edit ▸ Copy: the page's selection, as ⌘C copies it.
    let _ = copied(&mut d);
    choose(&mut app, &mut d, "Edit", "Copy");
    assert_eq!(copied(&mut d).as_deref(), Some("**loud**"));
    // Edit ▸ Undo: now the page's — the bold comes off.
    choose(&mut app, &mut d, "Edit", "Undo");
    assert_eq!(texts(&app)[0], "loud");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_menu_row_from_the_search_runs_once() {
    let (mut app, mut d, dir) = fresh_mac("menubar-search");
    key_with(&mut app, &mut d, "f", CTRL);
    let search = d.core.focus().unwrap();
    assert_eq!(app.fields.get(&search), Some(&Field::Search));
    // File ▸ New Note from the search: one note, not one from the row and
    // another from its key. (The open note has a title, so it stays.)
    app.store.notes[app.current].title = "Kept".into();
    let before = app.store.notes.len();
    choose(&mut app, &mut d, "File", "New Note");
    assert_eq!(app.store.notes.len(), before + 1);
    // Back in the search (a new note's title has the keyboard), Select
    // All and Cut are the field's own.
    key_with(&mut app, &mut d, "f", CTRL);
    assert_eq!(d.core.focus(), Some(search));
    d.text(&mut app, "milk");
    d.frame(&mut app);
    choose(&mut app, &mut d, "Edit", "Select All");
    let _ = copied(&mut d);
    choose(&mut app, &mut d, "Edit", "Cut");
    assert_eq!(d.core.edit_text(search).as_deref(), Some(""));
    assert_eq!(copied(&mut d).as_deref(), Some("milk"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_application_menu_is_the_macs_own() {
    let (app, d, dir) = fresh_mac("menubar-app");
    let bar = app.menu_bar(&d.core);
    let menu = &bar.menus[0];
    assert_eq!(menu.label, "Carnet");
    let roles: Vec<_> = menu.items.iter().map(|i| i.role).filter(|r| r.is_app()).collect();
    use kui_native::MenuRole::*;
    assert_eq!(roles, [About, Hide, HideOthers, ShowAll, Quit]);
    let named: Vec<_> = menu.items.iter().filter(|i| matches!(i.role, About | Hide | Quit)).map(|i| i.text()).collect();
    assert_eq!(named, ["About Carnet", "Hide Carnet", "Quit Carnet"]);
    // The Window menu is there for AppKit to fill.
    assert!(bar.menus.iter().any(|m| m.label == "Window"));
    let _ = std::fs::remove_dir_all(dir);
}

/// Settings' last line, quiet and faint: the welcome note again. The
/// first written in, a fresh one comes beside it at the top level, opened
/// and chosen in the library (every note shown, the folder it was on let
/// go), Settings closed. From the keyboard, a second time: that one,
/// untouched, not a third. On a Mac, Help ▸ Getting Started Note.
#[test]
fn the_welcome_note_comes_again_from_settings_and_the_help_menu() {
    let (mut app, mut d, dir) = fresh_mac("welcome-again");
    let first = app.store.notes.iter().position(|n| n.title == "Welcome to Carnet").expect("the welcome");
    app.store.notes[first].doc.push(Block::text("mine now"));
    app.store.notes[first].touch();
    app.store.save(first, true).unwrap();
    let first = app.store.notes[first].id;
    let work = app.store.create_folder("", "Work").unwrap();
    app.show_folder(Some(work));
    app.on_plain(Msg::ShowSettings);
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    // A button, named for a reader, in the faintest ink that reads.
    let k = d.key_named("Add the getting-started note").expect("the line, by its name");
    assert_eq!(d.key_of("add-welcome"), Some(k));
    let node = d.core.nodes().into_iter().find(|n| n.key == k).unwrap();
    assert_eq!(node.role, Some(Role::Button));
    assert!(node.flags.contains(&"click"), "{:?}", node.flags);
    // Its faint word reads on the page, and on its well under the pointer
    // and pressed, in every palette.
    for p in crate::design::Palette::ALL {
        for dark in [false, true] {
            let c = crate::design::Colors::of(p, dark);
            for (on, k) in [("page", 0.0), ("hover", 0.03), ("pressed", 0.04)] {
                let ratio = crate::design::contrast(c.faint, c.page.mix(c.ink, k));
                assert!(ratio >= 3.0, "{} {}: {ratio} on the {on}", p.name(), if dark { "dark" } else { "light" });
            }
        }
    }
    // Last of all: scrolled to, and clicked.
    for _ in 0..60 {
        if d.rect_of(k).is_some_and(|r| r.y > 60.0 && r.y + r.h < 740.0) {
            break;
        }
        d.wheel(&mut app, 700.0, 400.0, 0.0, -240.0);
        d.frame(&mut app);
    }
    let at = d.rect_of(k).expect("in the window").center();
    d.click(&mut app, at.x, at.y);
    d_frame_twice(&mut app, &mut d);
    assert!(!app.settings_open);
    let added = app.current;
    let note = &app.store.notes[added];
    assert_eq!((note.title.as_str(), note.folder.as_str()), ("Welcome to Carnet", ""));
    assert_eq!(note.path.as_deref(), Some(dir.join("Welcome to Carnet 2.md").as_path()), "beside the first, saved");
    assert_eq!(app.store.notes.len(), 2, "the first, and it");
    assert_eq!(app.folder, None, "every note shown");
    assert!(app.order.contains(&added), "its card in the library");
    // From the keyboard, again: the same one, untouched, not a third.
    app.on_plain(Msg::ShowSettings);
    d_frame_twice(&mut app, &mut d);
    let k = d.key_of("add-welcome").unwrap();
    d.focus(&mut app, k);
    d.frame(&mut app);
    assert_eq!(d.core.focus(), Some(k), "in the Tab ring");
    key(&mut app, &mut d, "enter");
    assert!(!app.settings_open);
    assert_eq!(app.current, added);
    assert_eq!(app.store.notes.len(), 2);
    // Help ▸ Getting Started Note, from the first: the same one opens.
    let i = app.store.notes.iter().position(|n| n.id == first).unwrap();
    app.on_plain(Msg::OpenNote { note: i as u64 });
    d.frame(&mut app);
    choose(&mut app, &mut d, "Help", "Getting Started Note");
    assert_eq!(app.current, added);
    assert_eq!(app.store.notes.len(), 2);
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_apps_chords_reach_it_from_the_title_and_the_search() {
    let (mut app, mut d, dir) = fresh("field-chords");
    d.text(&mut app, "Kept");
    d.frame(&mut app);
    let title = d.core.focus().unwrap();
    assert_eq!(app.fields.get(&title), Some(&Field::Title));
    // From the title: zen, once.
    key_with(&mut app, &mut d, ".", CTRL);
    assert!(app.zen);
    key_with(&mut app, &mut d, ".", CTRL);
    assert!(!app.zen);
    // From the title to the search.
    key_with(&mut app, &mut d, "f", CTRL);
    let search = d.core.focus().unwrap();
    assert_eq!(app.fields.get(&search), Some(&Field::Search));
    // From the search: a new note, one.
    let before = app.store.notes.len();
    key_with(&mut app, &mut d, "n", CTRL);
    assert_eq!(app.store.notes.len(), before + 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn down_from_the_title_is_the_note() {
    let (mut app, mut d, dir) = fresh("title-down");
    d.text(&mut app, "Plans");
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "First line");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "Second");
    // Up into the title, and down again: into the note, at its start.
    key_with(&mut app, &mut d, "up", CTRL);
    key(&mut app, &mut d, "up");
    d.frame(&mut app);
    let title = d.core.focus().unwrap();
    assert_eq!(app.fields.get(&title), Some(&Field::Title));
    key(&mut app, &mut d, "down");
    d.frame(&mut app);
    assert!(page_focused(&app, &d), "down from the title is the note");
    assert_eq!(app.caret, Caret::at(page::first(app.doc())));
    // → at the title's end: the same. Before its end, it moves in the
    // title.
    key(&mut app, &mut d, "up");
    d.frame(&mut app);
    assert_eq!(d.core.focus(), Some(title));
    key(&mut app, &mut d, "end");
    key(&mut app, &mut d, "left");
    key(&mut app, &mut d, "right");
    assert_eq!(d.core.focus(), Some(title), "→ inside the title stays there");
    key(&mut app, &mut d, "right");
    d.frame(&mut app);
    assert!(page_focused(&app, &d), "→ at the title's end is the note");
    assert_eq!(app.caret, Caret::at(page::first(app.doc())));
    // ↓ from the middle of the title: one press, into the note's first
    // line under the title's caret — not a walk to the title's end first.
    key_with(&mut app, &mut d, "up", CTRL);
    key(&mut app, &mut d, "up");
    d.frame(&mut app);
    key(&mut app, &mut d, "end");
    key(&mut app, &mut d, "left");
    key(&mut app, &mut d, "left");
    d.frame(&mut app);
    let column = d.core.edit_caret_rect(title).unwrap().x;
    key(&mut app, &mut d, "down");
    d.frame(&mut app);
    assert!(page_focused(&app, &d), "one ↓ from the middle is the note");
    assert_eq!(app.caret.head.slot, page::first(app.doc()).slot);
    assert!(app.caret.head.byte > 0, "not the line's start: under the title's caret");
    let at = app.rect_at(&d.core, app.caret.head.slot, app.caret.head.byte).unwrap().x;
    assert!((at - column).abs() < 12.0, "under the column: {at} vs {column}");
    assert_eq!(app.store.notes[app.current].title, "Plans", "the title as it was");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_colour_stays_its_colour_on_a_highlight_in_either_theme() {
    use crate::design::tint::{wash, HUES};
    for dark in [false, true] {
        let linen = crate::design::Colors::of(crate::design::Palette::Linen, dark);
        let (page, fg) = (linen.page, linen.ink);
        for ink in &HUES {
            for mark in &HUES {
                let c = crate::design::tint::ink((ink.ink << 8) | 0xff, dark, page);
                let w = wash(mark.wash, dark, page);
                assert!(crate::design::contrast(c, w) >= 3.5 || !dark, "{} on {}: readable in the dark", ink.name, mark.name);
                assert_eq!(super::page_view::words_ink(Some(c), fg, Some(w), page), c, "{} on {} ({})", ink.name, mark.name, if dark { "dark" } else { "light" });
            }
        }
    }
    // Black words on a dark page still give way; plain words on a wash
    // read.
    let linen = crate::design::Colors::of(crate::design::Palette::Linen, true);
    let (page, fg) = (linen.page, linen.ink);
    let black = crate::design::tint::ink(0x000000ff, true, page);
    assert_ne!(super::page_view::words_ink(Some(black), fg, None, page), black);
    // A colour from another app, too deep for the dark, is lightened
    // until it reads; on paper it is as written.
    let blue = crate::design::tint::ink(0x1f3a93ff, true, page);
    assert!(crate::design::contrast(blue, page) >= 4.5, "{blue:?}");
    assert_eq!(super::page_view::words_ink(Some(blue), fg, None, page), blue);
    assert_eq!(crate::design::tint::ink(0x1f3a93ff, false, page), Color::hex(0x1f3a93ff));
}

#[test]
fn the_search_is_quiet_until_it_is_used() {
    let (mut app, mut d, dir) = body("quiet-search");
    d.core.set_inspect(true);
    let pill = |d: &Drive| d.core.nodes().iter().find(|n| n.label.as_deref() == Some("search-pill")).map(|n| n.bg.a).unwrap();
    let settled = |app: &mut Carnet, d: &mut Drive| {
        settle(app, d);
        settle(app, d);
    };
    settled(&mut app, &mut d);
    let idle = pill(&d);
    // Mod+F: the field whole.
    key_with(&mut app, &mut d, "f", CTRL);
    settled(&mut app, &mut d);
    let used = pill(&d);
    assert!(idle < used * 0.5, "nothing searched, the keyboard elsewhere: quiet ({idle} against {used})");
    // A search keeps it whole when the keyboard goes back to the note.
    d.text(&mut app, "milk");
    settled(&mut app, &mut d);
    app.focus_page = true;
    settled(&mut app, &mut d);
    assert_eq!(pill(&d), used);
    // A window in the background: quiet again.
    d.core.set_focused(false);
    settled(&mut app, &mut d);
    assert!(pill(&d) < used * 0.5);
    let _ = std::fs::remove_dir_all(dir);
}

/// A row that means nothing where the keyboard is is greyed: the note's
/// block and format rows off the page, Undo with nothing to undo, Cut and
/// Copy with nothing selected. A greyed row's key goes on to the window.
#[test]
fn the_bar_greys_what_does_not_apply_where_the_keyboard_is() {
    let (mut app, mut d, dir) = fresh_mac("menubar-grey");
    let on = |app: &Carnet, d: &Drive, menu: &str, row: &str| {
        let bar = app.menu_bar(&d.core);
        let m = bar.menus.iter().find(|m| m.label == menu).expect(menu);
        m.items.iter().find(|i| i.text() == row).expect(row).enabled
    };
    // In the note, fresh: format rows yes, nothing to undo or copy yet.
    key(&mut app, &mut d, "tab");
    d.frame(&mut app);
    assert!(page_focused(&app, &d));
    assert!(on(&app, &d, "Format", "Bold"));
    assert!(on(&app, &d, "Edit", "Duplicate Block"));
    assert!(!on(&app, &d, "Edit", "Undo"));
    assert!(!on(&app, &d, "Edit", "Copy"));
    type_in(&mut app, &mut d, "loud");
    key_with(&mut app, &mut d, "home", SHIFT);
    assert!(on(&app, &d, "Edit", "Undo"));
    assert!(on(&app, &d, "Edit", "Copy"));
    // In the search: the note's rows grey, the field's own apply.
    key_with(&mut app, &mut d, "f", CTRL);
    assert_eq!(app.fields.get(&d.core.focus().unwrap()), Some(&Field::Search));
    assert!(!on(&app, &d, "Format", "Bold"));
    assert!(!on(&app, &d, "Edit", "Remove Block"));
    assert!(on(&app, &d, "Edit", "Paste"));
    assert!(!on(&app, &d, "Edit", "Undo"), "the search has typed nothing");
    assert!(on(&app, &d, "File", "New Note"));
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------ naming a folder

/// Presses the header's New folder and settles: the field that names it,
/// focused.
fn new_folder(app: &mut Carnet, d: &mut Drive) -> Key {
    let new = d.key_of("new-folder").expect("a new-folder button");
    d.click_key(app, new);
    d.frame(app);
    d.frame(app);
    let k = d.core.focus().expect("the name field has the keyboard");
    assert_eq!(app.fields.get(&k), Some(&Field::Folder), "the name field has the keyboard");
    k
}

/// A press in the middle of the page: the keyboard goes there.
fn click_page(app: &mut Carnet, d: &mut Drive) {
    let r = app.page_rect.expect("the page is laid out");
    d.click(app, r.x + r.w / 2.0, r.y + r.h / 2.0);
    d.frame(app);
    d.frame(app);
}

#[test]
fn a_folder_named_and_entered_is_made_and_shown() {
    let (mut app, mut d, dir) = fresh("name-enter");
    new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "Work");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["Work"]);
    assert_eq!(app.folder.as_deref(), Some("Work"));
    assert!(app.naming.is_none());
    assert!(d.core.focus().is_none(), "Enter lets go of the field");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn escape_drops_the_name_and_the_next_field_opens_empty() {
    let (mut app, mut d, dir) = fresh("name-escape");
    let first = new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "Old");
    key(&mut app, &mut d, "escape");
    assert!(app.store.folders.is_empty(), "no folder from an Escape");
    assert!(app.naming.is_none(), "the field is gone");
    // Again: an empty field, whatever kui kept of the last one, and what
    // is typed in it is what is made.
    let second = new_folder(&mut app, &mut d);
    assert_ne!(first, second);
    assert_eq!(d.core.edit_text(second).as_deref(), Some(""));
    type_in(&mut app, &mut d, "New");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["New"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_new_folder_field_shows_its_placeholder_until_typed_in() {
    let (mut app, mut d, dir) = fresh("name-placeholder");
    new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "Draft");
    key(&mut app, &mut d, "escape");
    let k = new_folder(&mut app, &mut d);
    // kui's own placeholder, which it draws only while the field's text
    // is empty, so it and the text never show together; not one drawn
    // over the field by the app.
    assert_eq!(d.core.edit.placeholder(k), Some("Folder name"));
    assert_eq!(d.core.edit_text(k).as_deref(), Some(""));
    assert!(!d.texts_under("folders").iter().any(|t| t == "Folder name"));
    type_in(&mut app, &mut d, "D");
    assert_eq!(d.core.edit_text(k).as_deref(), Some("D"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_named_folder_let_go_of_is_made_where_the_click_went() {
    let (mut app, mut d, dir) = fresh("name-blur");
    new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "Later");
    click_page(&mut app, &mut d);
    assert_eq!(app.store.folders, ["Later"], "made on blur");
    assert!(app.folder.is_none(), "and the library stays where it was");
    assert!(app.naming.is_none());
    assert!(dir.join("Later").is_dir());
    // An empty one let go of makes nothing.
    new_folder(&mut app, &mut d);
    click_page(&mut app, &mut d);
    assert_eq!(app.store.folders, ["Later"]);
    assert!(app.naming.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn enter_on_an_empty_name_makes_nothing_and_closes_the_field() {
    let (mut app, mut d, dir) = fresh("name-empty");
    new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "   ");
    key(&mut app, &mut d, "enter");
    assert!(app.store.folders.is_empty());
    assert!(app.naming.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn new_folder_while_naming_makes_the_first_and_opens_a_second() {
    let (mut app, mut d, dir) = fresh("name-twice");
    let first = new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "One");
    // The shortcut, from inside the field.
    key_with(&mut app, &mut d, "n", if cfg!(target_os = "macos") { CMD_SHIFT } else { CTRL_SHIFT });
    d.frame(&mut app);
    assert_eq!(app.store.folders, ["One"]);
    let second = d.core.focus().expect("the second field has the keyboard");
    assert_ne!(first, second);
    assert_eq!(app.fields.get(&second), Some(&Field::Folder));
    assert_eq!(d.core.edit_text(second).as_deref(), Some(""));
    type_in(&mut app, &mut d, "Two");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["One", "Two"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_subfolder_is_named_under_its_parent() {
    let (mut app, mut d, dir) = fresh("name-sub");
    new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "Work");
    key(&mut app, &mut d, "enter");
    app.folder_action("Work", "new");
    d.frame(&mut app);
    d.frame(&mut app);
    let k = d.core.focus().expect("the subfolder's field has the keyboard");
    assert_eq!(app.fields.get(&k), Some(&Field::Folder));
    type_in(&mut app, &mut d, "Plans");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["Work", "Work/Plans"]);
    assert_eq!(app.folder.as_deref(), Some("Work/Plans"));
    assert!(dir.join("Work").join("Plans").is_dir());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_name_the_disk_cannot_take_is_made_safe_and_an_existing_one_is_shown() {
    let (mut app, mut d, dir) = fresh("name-slug");
    new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "a/b: c");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["a-b- c"]);
    new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "a/b: c");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["a-b- c"], "not twice");
    assert_eq!(app.folder.as_deref(), Some("a-b- c"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_primary_modifier_with_backspace_in_a_field() {
    let (mut app, mut d, dir) = fresh("name-cmd-backspace");
    let k = new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "Work notes");
    key_with(&mut app, &mut d, "backspace", if cfg!(target_os = "macos") { CMD } else { CTRL });
    // ⌘⌫ takes the line back to its start; Ctrl+Backspace a word.
    let want = if cfg!(target_os = "macos") { "" } else { "Work " };
    assert_eq!(d.core.edit_text(k).as_deref(), Some(want));
    assert_eq!(app.naming.as_ref().map(|n| n.name.as_str()), Some(want), "and the name follows");
    let _ = std::fs::remove_dir_all(dir);
}

const CMD: KeyMods = KeyMods {
    shift: false,
    ctrl: false,
    alt: false,
    super_key: true,
};
const CMD_SHIFT: KeyMods = KeyMods {
    shift: true,
    ctrl: false,
    alt: false,
    super_key: true,
};

#[test]
fn a_block_selected_with_shift_down_moves_alone() {
    let (mut app, mut d, dir) = body("move-selected");
    for (i, s) in ["one", "two", "three"].iter().enumerate() {
        if i > 0 {
            key(&mut app, &mut d, "enter");
        }
        type_in(&mut app, &mut d, s);
    }
    // From the start of "two", Shift+↓: "two" whole, drawn selected, and
    // the selection's end at the start of "three", none of it taken.
    let two = page::block_slots(&app.doc().blocks()[1])[0];
    app.caret = Caret::at(page::start(two));
    d.frame(&mut app);
    key_with(&mut app, &mut d, "down", SHIFT);
    let cover = page::coverage(app.doc(), app.caret);
    assert!(cover.whole.contains(&app.doc().blocks()[1].id), "two is drawn selected");
    assert_eq!(page::touched(app.doc(), app.caret), 1..=1);
    // ⌥↓ moves what is drawn selected: "two", alone.
    key_with(&mut app, &mut d, "down", ALT);
    assert_eq!(texts(&app)[..3], ["one", "three", "two"]);
    let _ = std::fs::remove_dir_all(dir);
}

const ALT: KeyMods = KeyMods {
    shift: false,
    ctrl: false,
    alt: true,
    super_key: false,
};

// ------------------------------------------------------------ a folder row and the keyboard

/// Folders made from the store, drawn, and the keyboard on `name`'s row.
fn on_folder_row(name: &str, folders: &[&str]) -> (Carnet, Drive, std::path::PathBuf) {
    let (mut app, mut d, dir) = fresh(name);
    for f in folders {
        let (parent, leaf) = f.rsplit_once('/').unwrap_or(("", f));
        app.store.create_folder(parent, leaf).unwrap();
    }
    d.frame(&mut app);
    d.frame(&mut app);
    (app, d, dir)
}

fn focus_row(app: &mut Carnet, d: &mut Drive, row: FolderRow) {
    let k = app.folder_rows.iter().find(|(_, r)| *r == row).map(|(k, _)| *k).expect("the row is drawn");
    d.focus(app, k);
    d.frame(app);
}

fn focused_row(app: &Carnet, d: &Drive) -> Option<FolderRow> {
    let f = d.core.focus()?;
    app.folder_rows.iter().find(|(k, _)| *k == f).map(|(_, r)| r.clone())
}

fn folder(f: &str) -> FolderRow {
    FolderRow::Folder(f.to_string())
}

#[test]
fn f2_renames_a_folder_in_place_and_enter_keeps_the_keyboard_on_it() {
    let (mut app, mut d, dir) = on_folder_row("row-rename", &["Work", "Home"]);
    app.show_folder(Some("Work".into()));
    focus_row(&mut app, &mut d, folder("Work"));
    key(&mut app, &mut d, "f2");
    d.frame(&mut app);
    let k = d.core.focus().expect("the field has the keyboard");
    assert_eq!(app.fields.get(&k), Some(&Field::Folder));
    assert_eq!(d.core.edit_text(k).as_deref(), Some("Work"), "it opens on the name");
    // ⌘⌫ (Ctrl+Backspace elsewhere) clears it, then the new name.
    key_with(&mut app, &mut d, "backspace", if cfg!(target_os = "macos") { CMD } else { CTRL });
    type_in(&mut app, &mut d, "Job");
    key(&mut app, &mut d, "enter");
    d.frame(&mut app);
    assert_eq!(app.store.folders, ["Home", "Job"]);
    assert!(dir.join("Job").is_dir() && !dir.join("Work").exists());
    assert_eq!(app.folder.as_deref(), Some("Job"), "the library stays on it");
    assert_eq!(focused_row(&app, &d), Some(folder("Job")), "and the keyboard on its row");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_rename_dropped_or_emptied_keeps_the_name() {
    let (mut app, mut d, dir) = on_folder_row("row-rename-drop", &["Work"]);
    focus_row(&mut app, &mut d, folder("Work"));
    key(&mut app, &mut d, "f2");
    d.frame(&mut app);
    type_in(&mut app, &mut d, " more");
    key(&mut app, &mut d, "escape");
    d.frame(&mut app);
    assert_eq!(app.store.folders, ["Work"], "Escape keeps the name");
    assert_eq!(focused_row(&app, &d), Some(folder("Work")), "and goes back to the row");
    // Emptied, then Enter: the name it had.
    key(&mut app, &mut d, "f2");
    d.frame(&mut app);
    key_with(&mut app, &mut d, "backspace", if cfg!(target_os = "macos") { CMD } else { CTRL });
    if !cfg!(target_os = "macos") {
        key_with(&mut app, &mut d, "backspace", CTRL);
    }
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["Work"]);
    // A name another folder has: refused, said, and the name kept.
    app.store.create_folder("", "Home").unwrap();
    d.frame(&mut app);
    focus_row(&mut app, &mut d, folder("Work"));
    key(&mut app, &mut d, "f2");
    d.frame(&mut app);
    key_with(&mut app, &mut d, "backspace", if cfg!(target_os = "macos") { CMD } else { CTRL });
    type_in(&mut app, &mut d, "Home");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["Home", "Work"]);
    assert!(app.error.as_deref().is_some_and(|e| e.contains("rename")), "{:?}", app.error);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_rename_let_go_of_is_made() {
    let (mut app, mut d, dir) = on_folder_row("row-rename-blur", &["Work"]);
    app.folder_action("Work", "rename");
    d.frame(&mut app);
    d.frame(&mut app);
    type_in(&mut app, &mut d, "s");
    click_page(&mut app, &mut d);
    assert_eq!(app.store.folders, ["Works"]);
    assert!(app.naming.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn delete_on_a_folder_row_removes_it_and_the_keyboard_stays_in_the_list() {
    let (mut app, mut d, dir) = on_folder_row("row-delete", &["A", "B", "C"]);
    focus_row(&mut app, &mut d, folder("B"));
    key(&mut app, &mut d, "delete");
    assert_eq!(app.store.folders, ["A", "C"]);
    assert_eq!(focused_row(&app, &d), Some(folder("C")), "on the row that took its place");
    key(&mut app, &mut d, "backspace");
    assert_eq!(app.store.folders, ["A"]);
    assert_eq!(focused_row(&app, &d), Some(folder("A")), "the last gone: the one above");
    // "All notes" is not a folder to delete.
    focus_row(&mut app, &mut d, FolderRow::All);
    key(&mut app, &mut d, "delete");
    assert_eq!(app.store.folders, ["A"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn delete_on_a_folder_with_notes_trashes_it_and_undo_brings_it_back() {
    let (mut app, mut d, dir) = on_folder_row("row-trash", &["Work"]);
    app.show_folder(Some("Work".into()));
    app.new_note();
    d.frame(&mut app);
    d.text(&mut app, "Plan");
    d.frame(&mut app);
    app.save_current();
    focus_row(&mut app, &mut d, folder("Work"));
    key(&mut app, &mut d, "delete");
    assert!(app.store.folders.is_empty());
    assert!(!dir.join("Work").exists());
    assert!(app.notice.as_ref().is_some_and(|n| n.undo.is_some()), "with Undo");
    app.notice_undo();
    d.frame(&mut app);
    assert_eq!(app.store.folders, ["Work"]);
    assert!(dir.join("Work").join("Plan.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn up_and_down_walk_the_folder_rows() {
    let (mut app, mut d, dir) = on_folder_row("row-arrows", &["A", "A/Sub", "B"]);
    focus_row(&mut app, &mut d, FolderRow::All);
    key(&mut app, &mut d, "down");
    assert_eq!(focused_row(&app, &d), Some(folder("A")));
    assert_eq!(app.folder.as_deref(), Some("A"), "shown as it is reached");
    key(&mut app, &mut d, "down");
    assert_eq!(focused_row(&app, &d), Some(folder("A/Sub")));
    key(&mut app, &mut d, "down");
    key(&mut app, &mut d, "down");
    assert_eq!(focused_row(&app, &d), Some(folder("B")), "and no further");
    assert_eq!(app.folder.as_deref(), Some("B"));
    key(&mut app, &mut d, "up");
    assert_eq!(focused_row(&app, &d), Some(folder("A/Sub")));
    assert_eq!(app.folder.as_deref(), Some("A/Sub"));
    key(&mut app, &mut d, "up");
    key(&mut app, &mut d, "up");
    assert_eq!(focused_row(&app, &d), Some(FolderRow::All));
    assert!(app.folder.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_field_takes_option_and_command_moves() {
    let (mut app, mut d, dir) = fresh("field-moves");
    let k = new_folder(&mut app, &mut d);
    type_in(&mut app, &mut d, "one two three");
    let (word, line) = if cfg!(target_os = "macos") { (ALT, CMD) } else { (CTRL, CTRL) };
    let at = |app: &mut Carnet, d: &mut Drive, name: &str, mods: KeyMods| -> String {
        key_with(app, d, name, mods);
        type_in(app, d, "|");
        let t = d.core.edit_text(k).unwrap();
        key(app, d, "backspace");
        t
    };
    assert_eq!(at(&mut app, &mut d, "left", word), "one two |three");
    if cfg!(target_os = "macos") {
        assert_eq!(at(&mut app, &mut d, "left", line), "|one two three");
        assert_eq!(at(&mut app, &mut d, "right", line), "one two three|");
        assert_eq!(at(&mut app, &mut d, "up", word), "|one two three");
        assert_eq!(at(&mut app, &mut d, "down", word), "one two three|");
    } else {
        assert_eq!(at(&mut app, &mut d, "home", line), "|one two three");
        assert_eq!(at(&mut app, &mut d, "end", line), "one two three|");
    }
    key_with(&mut app, &mut d, "backspace", word);
    assert_eq!(d.core.edit_text(k).as_deref(), Some("one two "));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn f2_after_a_click_on_a_folder_renames_it() {
    let (mut app, mut d, dir) = on_folder_row("row-click-f2", &["Home", "Work"]);
    let k = app.folder_rows.iter().find(|(_, r)| *r == folder("Work")).unwrap().0;
    let r = d.rect_of(k).unwrap();
    d.click(&mut app, r.x + 20.0, r.y + r.h / 2.0);
    d.frame(&mut app);
    assert_eq!(app.folder.as_deref(), Some("Work"));
    key(&mut app, &mut d, "f2");
    d.frame(&mut app);
    type_in(&mut app, &mut d, "shop");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.store.folders, ["Home", "Workshop"]);
    assert_eq!(app.folder.as_deref(), Some("Workshop"));
    // Nothing shown but all notes, the keyboard on nothing: F2 renames
    // nothing.
    app.show_folder(None);
    d.core.set_focus(None);
    d.frame(&mut app);
    key(&mut app, &mut d, "f2");
    assert!(app.naming.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn up_and_down_after_a_click_on_a_folder_walk_from_it() {
    let (mut app, mut d, dir) = on_folder_row("row-click-arrows", &["Home", "Work"]);
    let k = app.folder_rows.iter().find(|(_, r)| *r == folder("Home")).unwrap().0;
    let r = d.rect_of(k).unwrap();
    d.click(&mut app, r.x + 20.0, r.y + r.h / 2.0);
    d.frame(&mut app);
    assert_eq!(app.folder.as_deref(), Some("Home"));
    key(&mut app, &mut d, "down");
    assert_eq!(app.folder.as_deref(), Some("Work"));
    assert_eq!(focused_row(&app, &d), Some(folder("Work")), "and the keyboard is on it now");
    // Into the note: the arrows are the note's again.
    click_page(&mut app, &mut d);
    key(&mut app, &mut d, "up");
    assert_eq!(app.folder.as_deref(), Some("Work"));
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------ folding folders

/// A folder's chevron: the one in its row.
fn chevron(d: &mut Drive, app: &mut Carnet, f: &str) -> Option<Key> {
    d.core.set_inspect(true);
    d.frame(app);
    let row = app.folder_rects.iter().find(|(r, _)| r == f)?.1;
    d.core.nodes().iter().filter(|n| n.label.as_deref() == Some("fold")).map(|n| n.key).find(|&k| {
        d.rect_of(k).is_some_and(|r| {
            let c = r.center();
            c.y >= row.y && c.y <= row.y + row.h
        })
    })
}

/// Whether a reader hears folder row `name` as open, shut, or neither.
fn heard_open(d: &mut Drive, name: &str) -> Option<bool> {
    d.core.access_tree().nodes.iter().find(|n| n.name.as_deref() == Some(name)).and_then(|n| n.expanded)
}

fn click_chevron(app: &mut Carnet, d: &mut Drive, f: &str, mods: KeyMods) {
    let k = chevron(d, app, f).unwrap_or_else(|| panic!("a chevron for {f}"));
    let r = d.rect_of(k).unwrap();
    d.input(app, InputEvent::Modifiers(mods));
    d.click(app, r.x + r.w / 2.0, r.y + r.h / 2.0);
    d.input(app, InputEvent::Modifiers(KeyMods::NONE));
    d_frame_twice(app, d);
}

/// A folder's chevron is an easy target: the row's height, and as wide as
/// the indent lets it be, the folder's icon where it would be without one;
/// a click on the folder's name beside it chooses the folder, not a fold.
#[test]
fn a_folders_chevron_is_an_easy_target() {
    let (mut app, mut d, dir) = on_folder_row("fold-target", &["A", "A/Sub", "B"]);
    let k = chevron(&mut d, &mut app, "A").expect("A's chevron");
    let r = d.rect_of(k).unwrap();
    let row = folder_row(&app, "A");
    assert!(r.w >= 19.0 && r.h >= 30.0, "{r:?}");
    let name_x = |d: &Drive, f: &str| d.core.nodes().iter().find(|n| n.text.as_deref() == Some(f)).map(|n| n.rect.x).unwrap();
    assert_eq!(name_x(&d, "A"), name_x(&d, "B"), "the names line up, chevron or none");
    // A click at the chevron's edge folds; one on the name chooses.
    d.click(&mut app, r.x + 2.0, r.y + r.h / 2.0);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(folded(&app), ["A"]);
    d.click(&mut app, r.x + r.w + 40.0, row.y);
    d_frame_twice(&mut app, &mut d);
    assert_eq!((folded(&app), app.folder.as_deref()), (vec!["A"], Some("A")));
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: a folded folder whose notes are all in its subfolders
/// read 0. Folded, its count takes in the notes of the folders it hides;
/// open, it's its own, as theirs show beside them.
#[test]
fn a_folded_folders_count_takes_in_its_hidden_folders() {
    let (mut app, mut d, dir) = library("fold-count", &[("Home/Sub/Deep", "x"), ("Home/Sub/Deeper", "y"), ("Home/Other/Z", "z")]);
    d_frame_twice(&mut app, &mut d);
    let count = |app: &Carnet, d: &Drive, f: &str| -> String {
        let row = app.folder_rects.iter().find(|(r, _)| r == f).map(|(_, r)| *r).unwrap();
        d.core
            .nodes()
            .iter()
            .filter(|n| n.rect.y >= row.y && n.rect.y + n.rect.h <= row.y + row.h + 0.5 && n.rect.x >= row.x && n.rect.x <= row.x + row.w)
            .filter_map(|n| n.text.clone())
            .find(|t| t.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or_default()
    };
    assert_eq!(count(&app, &d, "Home"), "0", "open: its own");
    click_chevron(&mut app, &mut d, "Home", KeyMods::NONE);
    assert_eq!(folded(&app), ["Home"]);
    assert_eq!(count(&app, &d, "Home"), "3", "folded: the notes it hides too");
    click_chevron(&mut app, &mut d, "Home", KeyMods::NONE);
    assert_eq!((count(&app, &d, "Home"), count(&app, &d, "Home/Sub")), ("0".to_string(), "2".to_string()), "open again: each its own");
    let mut counts = HashMap::new();
    counts.insert("A", 1);
    counts.insert("A/B", 2);
    counts.insert("AB", 5);
    assert_eq!((super::chrome::folder_count(&counts, "A", true), super::chrome::folder_count(&counts, "A", false)), (3, 1));
    let _ = std::fs::remove_dir_all(dir);
}

/// The folder rows drawn, top to bottom.
fn rows_drawn(app: &Carnet) -> Vec<String> {
    app.folder_rows
        .iter()
        .filter_map(|(_, r)| match r {
            FolderRow::Folder(f) => Some(f.clone()),
            _ => None,
        })
        .collect()
}

fn folded(app: &Carnet) -> Vec<&str> {
    app.settings.collapsed.iter().map(String::as_str).collect()
}

/// A folder with folders inside has a chevron; a click on it hides them
/// and shows them again, the folder shown staying as it was and its count
/// as it was. A folder with none inside has no chevron.
#[test]
fn a_folders_chevron_folds_its_subfolders_away_and_back() {
    let (mut app, mut d, dir) = on_folder_row("fold-click", &["A", "A/Sub", "A/Sub/Deep", "B"]);
    app.show_folder(Some("B".into()));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(rows_drawn(&app), ["A", "A/Sub", "A/Sub/Deep", "B"]);
    assert!(chevron(&mut d, &mut app, "B").is_none(), "nothing inside B: no chevron");
    assert!(chevron(&mut d, &mut app, "A/Sub/Deep").is_none());
    assert!(chevron(&mut d, &mut app, "A").is_some());
    assert_eq!(heard_open(&mut d, "A"), Some(true), "a reader hears it open");
    assert_eq!(heard_open(&mut d, "B"), None);
    click_chevron(&mut app, &mut d, "A", KeyMods::NONE);
    assert_eq!(rows_drawn(&app), ["A", "B"]);
    assert_eq!(folded(&app), ["A"]);
    assert_eq!(app.folder.as_deref(), Some("B"), "the chevron only folds: the folder shown stays");
    assert!(app.folder_rects.iter().all(|(f, _)| !f.starts_with("A/")), "nor rows for a drag");
    assert_eq!(heard_open(&mut d, "A"), Some(false), "and shut now");
    // The keyboard is not held in the chevron.
    assert!(d.core.focus().is_none_or(|k| k == Key::ROOT), "{:?}", d.core.focus());
    click_chevron(&mut app, &mut d, "A", KeyMods::NONE);
    assert_eq!(rows_drawn(&app), ["A", "A/Sub", "A/Sub/Deep", "B"]);
    assert!(folded(&app).is_empty());
    // A click on the folder's name still shows it, and folds nothing.
    let at = folder_row(&app, "A");
    d.click(&mut app, at.x, at.y);
    d.frame(&mut app);
    assert_eq!(app.folder.as_deref(), Some("A"));
    assert!(folded(&app).is_empty());
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// ⌥-click (Alt) on a chevron folds the folder and every folder inside
/// it, and unfolds them all again.
#[test]
fn an_option_click_folds_every_folder_inside() {
    let (mut app, mut d, dir) = on_folder_row("fold-alt", &["A", "A/Sub", "A/Sub/Deep", "A/Two", "A/Two/More", "B"]);
    click_chevron(&mut app, &mut d, "A", ALT);
    assert_eq!(folded(&app), ["A", "A/Sub", "A/Two"], "only folders with folders inside");
    assert_eq!(rows_drawn(&app), ["A", "B"]);
    // Unfolded alone: what is inside stays folded.
    click_chevron(&mut app, &mut d, "A", KeyMods::NONE);
    assert_eq!(rows_drawn(&app), ["A", "A/Sub", "A/Two", "B"]);
    click_chevron(&mut app, &mut d, "A", ALT);
    assert_eq!(folded(&app), ["A", "A/Sub", "A/Two"]);
    click_chevron(&mut app, &mut d, "A", ALT);
    assert!(folded(&app).is_empty());
    assert_eq!(rows_drawn(&app), ["A", "A/Sub", "A/Sub/Deep", "A/Two", "A/Two/More", "B"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// ← and → on a folder's row, as in Finder: → unfolds, then goes in; ←
/// folds, then goes up to the folder it is in. ⌥ with them, every folder
/// inside too.
#[test]
fn left_and_right_fold_unfold_and_walk_the_tree() {
    let (mut app, mut d, dir) = on_folder_row("fold-keys", &["A", "A/Sub", "A/Sub/Deep", "B"]);
    focus_row(&mut app, &mut d, folder("A"));
    key(&mut app, &mut d, "left");
    assert_eq!(folded(&app), ["A"]);
    assert_eq!(rows_drawn(&app), ["A", "B"]);
    assert_eq!(focused_row(&app, &d), Some(folder("A")), "the keyboard stays on it");
    // Folded, at the top: ← goes up to All notes, → back down to A.
    key(&mut app, &mut d, "left");
    assert_eq!((focused_row(&app, &d), app.folder.as_deref()), (Some(FolderRow::All), None));
    key(&mut app, &mut d, "right");
    assert_eq!((focused_row(&app, &d), app.folder.as_deref()), (Some(folder("A")), Some("A")));
    key(&mut app, &mut d, "right");
    assert!(folded(&app).is_empty());
    assert_eq!(focused_row(&app, &d), Some(folder("A")));
    // Open: → goes to the first folder inside, and shows it.
    key(&mut app, &mut d, "right");
    assert_eq!(focused_row(&app, &d), Some(folder("A/Sub")));
    assert_eq!(app.folder.as_deref(), Some("A/Sub"));
    key(&mut app, &mut d, "right");
    key(&mut app, &mut d, "right");
    assert_eq!(focused_row(&app, &d), Some(folder("A/Sub/Deep")));
    // Nothing inside: → does nothing; ← goes up.
    key(&mut app, &mut d, "right");
    assert_eq!(focused_row(&app, &d), Some(folder("A/Sub/Deep")));
    key(&mut app, &mut d, "left");
    assert_eq!(focused_row(&app, &d), Some(folder("A/Sub")));
    assert_eq!(app.folder.as_deref(), Some("A/Sub"));
    key(&mut app, &mut d, "left");
    assert_eq!(folded(&app), ["A/Sub"]);
    key(&mut app, &mut d, "left");
    assert_eq!(focused_row(&app, &d), Some(folder("A")));
    // ⌥←: A and all inside it folded; ⌥→: all open.
    key_with(&mut app, &mut d, "right", ALT);
    assert!(folded(&app).is_empty(), "{:?}", folded(&app));
    key_with(&mut app, &mut d, "left", ALT);
    assert_eq!(folded(&app), ["A", "A/Sub"]);
    // After a click on the folder, the keyboard on nothing in
    // particular: the same, from the folder shown.
    let at = folder_row(&app, "A");
    d.click(&mut app, at.x, at.y);
    d.frame(&mut app);
    key(&mut app, &mut d, "right");
    assert_eq!(folded(&app), ["A/Sub"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// All notes has a chevron once there is a folder, every folder being
/// inside it: a click folds them all away, the folder shown giving way to
/// All notes, and back; Deleted stays. Its name sits a level out from the
/// folders'. ← and → on its row do the same, and → then goes in.
#[test]
fn all_notes_folds_every_folder_away_and_back() {
    let (mut app, mut d, dir) = on_folder_row("fold-all", &["A", "A/Sub", "B"]);
    // Something deleted, for a Deleted row: the welcome note (an empty
    // one is not kept).
    let welcome = app.store.notes.iter().position(|n| !n.markdown().trim().is_empty()).unwrap();
    app.delete_note_at(welcome);
    app.show_folder(Some("A/Sub".into()));
    d_frame_twice(&mut app, &mut d);
    assert!(chevron(&mut d, &mut app, "").is_some(), "All notes' chevron");
    assert_eq!(heard_open(&mut d, "All notes"), Some(true));
    let name_x = |d: &Drive, f: &str| d.core.nodes().iter().find(|n| n.text.as_deref() == Some(f)).map(|n| n.rect.x).unwrap();
    assert_eq!(name_x(&d, "A") - name_x(&d, "All notes"), 14.0, "the folders a level in");
    assert_eq!(name_x(&d, "Deleted"), name_x(&d, "All notes"), "Deleted level with All notes");
    click_chevron(&mut app, &mut d, "", KeyMods::NONE);
    assert_eq!(folded(&app), [""]);
    assert!(rows_drawn(&app).is_empty(), "{:?}", rows_drawn(&app));
    assert_eq!(app.folder, None, "the folder shown was inside: All notes shown");
    assert!(app.folder_rows.iter().any(|(_, r)| *r == FolderRow::Deleted), "Deleted is not inside it");
    assert_eq!(heard_open(&mut d, "All notes"), Some(false));
    click_chevron(&mut app, &mut d, "", KeyMods::NONE);
    assert!(folded(&app).is_empty());
    assert_eq!(rows_drawn(&app), ["A", "A/Sub", "B"]);
    // From the keyboard: ← folds, → unfolds, → again goes to the first
    // folder.
    focus_row(&mut app, &mut d, FolderRow::All);
    key(&mut app, &mut d, "left");
    assert_eq!((folded(&app), rows_drawn(&app).len()), (vec![""], 0));
    assert_eq!(focused_row(&app, &d), Some(FolderRow::All), "the keyboard stays on it");
    key(&mut app, &mut d, "right");
    assert!(folded(&app).is_empty());
    key(&mut app, &mut d, "right");
    assert_eq!(focused_row(&app, &d), Some(folder("A")));
    // A new folder at the top, All notes folded: unfolded for its field.
    focus_row(&mut app, &mut d, FolderRow::All);
    key(&mut app, &mut d, "left");
    app.name_folder("");
    d_frame_twice(&mut app, &mut d);
    assert!(folded(&app).is_empty(), "{:?}", folded(&app));
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// ↑ and ↓ walk only the rows in sight, and ⇧ with them (and a ⇧-click's
/// range) chooses only those: a folded folder's subfolders are passed.
#[test]
fn the_arrows_and_a_range_pass_folded_folders_by() {
    let (mut app, mut d, dir) = on_folder_row("fold-arrows", &["A", "A/Sub", "B", "C"]);
    focus_row(&mut app, &mut d, folder("A"));
    key(&mut app, &mut d, "left");
    focus_row(&mut app, &mut d, FolderRow::All);
    key(&mut app, &mut d, "down");
    assert_eq!(focused_row(&app, &d), Some(folder("A")));
    key(&mut app, &mut d, "down");
    assert_eq!(focused_row(&app, &d), Some(folder("B")), "past A/Sub, folded away");
    key(&mut app, &mut d, "up");
    assert_eq!(focused_row(&app, &d), Some(folder("A")));
    key_with(&mut app, &mut d, "down", SHIFT);
    assert_eq!(app.picked, [Pick::Folder("A".into()), Pick::Folder("B".into())]);
    // A ⇧-click from A to C: A, B and C, not what A hides.
    app.picked.clear();
    let (a, c) = (folder_row(&app, "A"), folder_row(&app, "C"));
    d.click(&mut app, a.x, a.y);
    d.input(&mut app, InputEvent::Modifiers(SHIFT));
    d.click(&mut app, c.x, c.y);
    d.input(&mut app, InputEvent::Modifiers(KeyMods::NONE));
    d.frame(&mut app);
    assert_eq!(app.picked, ["A", "B", "C"].map(|f| Pick::Folder(f.into())));
    let _ = std::fs::remove_dir_all(dir);
}

/// The folder shown is never out of sight: shown however it came to be
/// (a search's note, a move, a link, by hand), the folders it is in
/// unfold; one folded with the folder shown inside it is shown instead;
/// a new folder named inside a folded one unfolds it.
#[test]
fn the_folder_shown_is_never_folded_away() {
    let (mut app, mut d, dir) = on_folder_row("fold-reveal", &["A", "A/Sub", "A/Sub/Deep", "B", "B/In"]);
    focus_row(&mut app, &mut d, folder("A"));
    key_with(&mut app, &mut d, "left", ALT);
    assert_eq!(folded(&app), ["A", "A/Sub"]);
    app.show_folder(Some("A/Sub/Deep".into()));
    d_frame_twice(&mut app, &mut d);
    assert!(folded(&app).is_empty(), "{:?}", folded(&app));
    assert!(rows_drawn(&app).contains(&"A/Sub/Deep".to_string()));
    // Folding a folder the one shown is inside: that one is shown.
    click_chevron(&mut app, &mut d, "A", KeyMods::NONE);
    assert_eq!(app.folder.as_deref(), Some("A"));
    assert_eq!(folded(&app), ["A"]);
    // A new folder inside a folded one: its field in sight, and it shown
    // once made.
    click_chevron(&mut app, &mut d, "B", KeyMods::NONE);
    assert_eq!(folded(&app), ["A", "B"]);
    app.name_folder("B/In");
    d_frame_twice(&mut app, &mut d);
    assert_eq!(folded(&app), ["A"], "B unfolded for the field");
    type_in(&mut app, &mut d, "New");
    key(&mut app, &mut d, "enter");
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.folder.as_deref(), Some("B/In/New"));
    assert!(rows_drawn(&app).contains(&"B/In/New".to_string()), "{:?}", rows_drawn(&app));
    let _ = std::fs::remove_dir_all(dir);
}

/// A folder renamed or moved stays folded, and the folders inside it.
#[test]
fn a_folder_renamed_or_moved_stays_folded() {
    let (mut app, mut d, dir) = on_folder_row("fold-rename", &["A", "A/Sub", "A/Sub/Deep", "B"]);
    focus_row(&mut app, &mut d, folder("A"));
    key_with(&mut app, &mut d, "left", ALT);
    app.rename_folder("A", "Z");
    d_frame_twice(&mut app, &mut d);
    assert_eq!(folded(&app), ["Z", "Z/Sub"]);
    app.move_folder("Z", "B");
    d_frame_twice(&mut app, &mut d);
    assert_eq!(folded(&app), ["B/Z", "B/Z/Sub"]);
    assert_eq!(rows_drawn(&app), ["B", "B/Z"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// What is folded is this device's: kept in its settings file, read back
/// at the next start; a file from before it reads as all open.
#[test]
fn folded_folders_stay_folded_after_a_restart() {
    let (store, dir) = temp_store("fold-restart");
    let file = dir.with_extension("settings.txt");
    let _ = std::fs::remove_file(&file);
    let mut app = Carnet::with_settings(store, Settings { file: Some(file.clone()), ..Settings::default() });
    for f in ["A", "A/Sub", "B"] {
        let (parent, leaf) = f.rsplit_once('/').unwrap_or(("", f));
        app.store.create_folder(parent, leaf).unwrap();
    }
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d_frame_twice(&mut app, &mut d);
    click_chevron(&mut app, &mut d, "A", KeyMods::NONE);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("collapsed = A\n"), "{text}");
    drop(app);
    // The next start: the same notes, the settings read back.
    let mut app = Carnet::with_settings(Store::open(dir.clone()).unwrap(), Settings { file: Some(file.clone()), ..Settings::parse(&text) });
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d_frame_twice(&mut app, &mut d);
    assert_eq!(rows_drawn(&app), ["A", "B"]);
    click_chevron(&mut app, &mut d, "A", KeyMods::NONE);
    assert!(!std::fs::read_to_string(&file).unwrap().contains("collapsed"));
    assert!(Settings::parse("theme = dark\n").collapsed.is_empty(), "an older file: all open");
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_dir_all(dir);
}

/// A note or folder carried over a folded folder, held there a moment:
/// it unfolds, and what is carried can go deeper. Passing over does not.
#[test]
fn a_folded_folder_springs_open_under_a_drag() {
    let (mut app, mut d, dir) = library("fold-spring", &[("Work/Plan", "q3"), ("Home/Sub/Deep", "x"), ("Home/Pantry", "rice")]);
    focus_row(&mut app, &mut d, folder("Home"));
    key(&mut app, &mut d, "left");
    assert_eq!(folded(&app), ["Home"]);
    let (work, home) = (folder_row(&app, "Work"), folder_row(&app, "Home"));
    d.input(&mut app, InputEvent::CursorMoved(work));
    d.input(&mut app, InputEvent::mouse_down(1));
    d.move_to(&mut app, home.x, home.y);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(folded(&app), ["Home"], "not at once");
    std::thread::sleep(SPRING + Duration::from_millis(50));
    d_frame_twice(&mut app, &mut d);
    assert!(folded(&app).is_empty(), "held there, it opens");
    let sub = folder_row(&app, "Home/Sub");
    d.move_to(&mut app, sub.x, sub.y);
    d_frame_twice(&mut app, &mut d);
    d.input(&mut app, InputEvent::mouse_up());
    d_frame_twice(&mut app, &mut d);
    assert!(dir.join("Home/Sub/Work/Plan.md").exists(), "dropped deeper: {:?}", app.store.folders);
    // Only passing over a folded folder: it stays folded.
    focus_row(&mut app, &mut d, folder("Home/Sub"));
    key(&mut app, &mut d, "left");
    assert_eq!(folded(&app), ["Home/Sub"]);
    let (pantry, sub) = (folder_row(&app, ""), folder_row(&app, "Home/Sub"));
    d.input(&mut app, InputEvent::CursorMoved(pantry));
    d.input(&mut app, InputEvent::mouse_down(1));
    d.move_to(&mut app, sub.x, sub.y);
    d_frame_twice(&mut app, &mut d);
    d.move_to(&mut app, pantry.x, pantry.y + 2.0);
    d_frame_twice(&mut app, &mut d);
    std::thread::sleep(SPRING + Duration::from_millis(50));
    d_frame_twice(&mut app, &mut d);
    d.input(&mut app, InputEvent::mouse_up());
    d_frame_twice(&mut app, &mut d);
    assert_eq!(folded(&app), ["Home/Sub"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// What a server says of itself, listing these features.
fn server_info(features: &[&str]) -> carnet_sync::info::ServerInfo {
    carnet_sync::info::ServerInfo { greeting: carnet_sync::compat::Greeting::ours(features.iter().map(|f| f.to_string()).collect()), ..Default::default() }
}

/// Whether a drawn node reads `s`. (The inspector gives a long text's
/// first 60 characters only: the access tree has it whole.)
fn drawn(d: &mut Drive, app: &mut Carnet, s: &str) -> bool {
    d.core.set_inspect(true);
    d.frame(app);
    d.core.nodes().iter().any(|n| n.text.as_deref() == Some(s)) || d.core.access_tree().nodes.iter().any(|n| n.name.as_deref() == Some(s))
}

/// A note saved twice: "First words", then "First words and more".
fn two_versions(name: &str) -> (Carnet, Drive, std::path::PathBuf) {
    let (mut app, mut d, dir) = body(name);
    type_in(&mut app, &mut d, "First words");
    app.save_current();
    type_in(&mut app, &mut d, " and more");
    app.save_current();
    (app, d, dir)
}

fn remove_test_dir(dir: std::path::PathBuf) {
    let _ = std::fs::remove_dir_all(dir.with_extension("replica"));
    let _ = std::fs::remove_dir_all(dir);
}

/// The web app's line over its history, a click for a tap.
const HISTORY_HINT_WORDS: [&str; 3] = ["Click one to read it.", "added words in green", "removed ones struck through in red"];

#[test]
fn a_notes_history_shows_its_versions_read_only_and_brings_one_back() {
    let (mut app, mut d, dir) = two_versions("history");
    let now = texts(&app);
    assert_eq!(now[0], "First words and more");

    app.on_plain(Msg::ShowHistory);
    d.frame(&mut app);
    let v = app.versions.as_ref().expect("the history shows");
    let origins: Vec<_> = v.list.iter().map(|r| r.origin).collect();
    assert_eq!(origins, [Some(carnet_sync::Origin::Save), Some(carnet_sync::Origin::Import)]);
    // It opens on the note as it is, its last change marked: the words
    // the last save added, washed green.
    assert_eq!(texts(&app)[0], "First words <mark style=\"background: #D8EAC0CC\">and more</mark>");
    // Listed as the web app lists them: what made each, when, and the
    // words it changed, under a line saying what the marks mean.
    // (The snapshot keeps a node's first 60 characters.)
    d.frame(&mut app);
    assert!(
        d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.starts_with("Click one to read it. What changed is marked"))),
        "how to read them"
    );
    assert!(HISTORY_HINT_WORDS.iter().all(|w| super::chrome::HISTORY_HINT.contains(w)));
    assert!(!drawn(&mut d, &mut app, "added"), "the list's line says what the marks mean");
    // In the library's place, the whole column: no search, folders or
    // notes over or under it.
    assert!(drawn(&mut d, &mut app, "History"));
    no_library(&mut d, &mut app);
    assert!(drawn(&mut d, &mut app, "Edited here"));
    assert!(drawn(&mut d, &mut app, "The note as it is · +2 words"));
    assert!(drawn(&mut d, &mut app, "Where its history starts"));
    assert!(d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.ends_with(" · +2 words") && t != "The note as it is · +2 words")), "the first version: when, and its words");
    assert!(drawn(&mut d, &mut app, "Done"));
    assert!(!drawn(&mut d, &mut app, "Restore"), "the note as it is has nothing to restore");

    app.on_plain(Msg::ShowVersion { index: 1 });
    d.frame(&mut app);
    // Where its history starts: nothing before it to mark against.
    assert_eq!(texts(&app)[0], "First words");
    assert!(drawn(&mut d, &mut app, "Restore"));
    // Against now: what restoring it would take away, struck.
    app.on_plain(Msg::CompareWithNow);
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "First words ~~<span style=\"color: #C0553F\">and more</span>~~");
    assert!(drawn(&mut d, &mut app, "goes"));
    app.on_plain(Msg::CompareWithNow);
    d.frame(&mut app);
    assert_eq!(texts(&app)[0], "First words");
    // Read, not written: typing, Backspace and undo leave it be.
    type_in(&mut app, &mut d, "typed into a version");
    key(&mut app, &mut d, "backspace");
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(texts(&app)[0], "First words");
    assert!(!app.store.notes[app.current].dirty);
    assert_eq!(app.store.notes[app.current].doc.blocks()[0].text_ref(), Some("First words and more"));

    app.on_plain(Msg::RestoreVersion);
    d.frame(&mut app);
    assert!(app.versions.is_none(), "back to the note");
    assert_eq!(texts(&app)[0], "First words");
    let path = app.store.notes[app.current].path.clone().unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("First words\n"));
    assert!(app.notice.as_ref().is_some_and(|n| n.text.starts_with("Restored the version from")));

    // The notice's Undo is a restore of the version before it.
    app.on_plain(Msg::Undo);
    d.frame(&mut app);
    assert_eq!(texts(&app), now);
    assert!(std::fs::read_to_string(&path).unwrap().contains("First words and more"));
    let list = app.store.history(app.current);
    assert_eq!(list[0].origin, Some(carnet_sync::Origin::Restore));
    remove_test_dir(dir);
}

#[test]
fn a_version_restored_as_a_copy_is_a_note_of_its_own() {
    let (mut app, mut d, dir) = two_versions("history-copy");
    let original = app.store.notes[app.current].id;
    app.on_plain(Msg::ShowHistory);
    app.on_plain(Msg::ShowVersion { index: 1 });
    app.on_plain(Msg::RestoreVersionCopy);
    d.frame(&mut app);
    assert!(app.versions.is_none());
    let copy = &app.store.notes[app.current];
    assert_ne!(copy.id, original);
    assert_eq!(copy.title, "Untitled (restored)");
    assert_eq!(texts(&app)[0], "First words");
    assert!(copy.path.as_ref().is_some_and(|p| p.exists()), "saved");
    let left = app.store.notes.iter().find(|n| n.id == original).unwrap();
    assert_eq!(left.doc.blocks()[0].text_ref(), Some("First words and more"), "the note is left as it is");
    remove_test_dir(dir);
}

#[test]
fn the_history_goes_away_with_esc_another_note_or_the_menus() {
    let (mut app, mut d, dir) = two_versions("history-leave");
    app.on_plain(Msg::ShowHistory);
    app.on_plain(Msg::ShowVersion { index: 1 });
    d.frame(&mut app);
    key(&mut app, &mut d, "escape");
    assert!(app.versions.is_none());
    assert_eq!(texts(&app)[0], "First words and more", "the page is the note again");

    // From the note's menu in the library, and gone on opening another.
    let id = app.store.notes[app.current].id;
    assert!(app.note_menu(id).iter().any(|i| format!("{i:?}").contains("\"history\"")));
    app.on_plain(Msg::ShowHistory);
    assert!(app.versions.is_some());
    let other = (0..app.store.notes.len()).find(|&i| i != app.current).unwrap();
    app.open(other);
    assert!(app.versions.is_none());

    // A note with no history yet says so, and nothing opens.
    app.new_note();
    app.on_plain(Msg::ShowHistory);
    assert!(app.versions.is_none());
    remove_test_dir(dir);
}

/// Forty notes in a folder "Work", and a new note at the top level saved
/// twice ("First words", then "First words and more"): a library long
/// enough to scroll, and a note with a history.
fn a_library_and_two_versions(name: &str, w: f32) -> (Carnet, Drive, std::path::PathBuf) {
    let (store, dir) = temp_store(name);
    drop(store);
    std::fs::create_dir_all(dir.join("Work")).unwrap();
    for i in 0..40 {
        std::fs::write(dir.join("Work").join(format!("Note {i}.md")), format!("# Note {i}\n\nWords of note {i}\n")).unwrap();
    }
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), w, 760.0).framing();
    d.frame(&mut app);
    app.new_note();
    d_frame_twice(&mut app, &mut d);
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "First words");
    app.save_current();
    type_in(&mut app, &mut d, " and more");
    app.save_current();
    d.frame(&mut app);
    (app, d, dir)
}

/// Where the last frame put the node keyed `label`.
fn keyed_rect(d: &mut Drive, label: &str) -> Rect {
    let k = d.key_of(label).unwrap_or_else(|| panic!("{label} is drawn"));
    d.rect_of(k).unwrap()
}

/// Nothing of the library is drawn: not its top row's title, its search,
/// its folders or its notes.
fn no_library(d: &mut Drive, app: &mut Carnet) {
    d.frame(app);
    for k in ["library", "folders", "search-pill", "new-folder", "new-note", "open-settings"] {
        assert!(d.key_of(k).is_none(), "{k} is not drawn with the history");
    }
    assert!(!drawn(d, app, "All notes"), "nor its folders' names");
    assert!(!drawn(d, app, "Notes"));
}

#[test]
fn the_history_is_a_level_of_its_own_and_back_leaves_the_library_as_it_was() {
    let (mut app, mut d, dir) = a_library_and_two_versions("history-level", 1100.0);
    app.show_folder(Some("Work".into()));
    app.focus(Field::Search);
    d.frame(&mut app);
    type_in(&mut app, &mut d, "Note");
    d_frame_twice(&mut app, &mut d);
    let search = d.core.focus().expect("the search has the keyboard");
    let count = app.order.len();
    assert!(count >= 40, "the notes in Work, and any other the search finds");
    let list = d.key_of("library").expect("the notes are listed");
    d.core.set_scroll(list, Vec2::new(0.0, 400.0));
    d_frame_twice(&mut app, &mut d);
    let scrolled = d.core.scroll_offset(list).y;
    assert!(scrolled > 300.0, "scrolled down the notes: {scrolled}");
    let width = keyed_rect(&mut d, "library-pane").w;

    for back in ["escape", "the back button", "Done"] {
        app.on_plain(Msg::ShowHistory);
        d_frame_twice(&mut app, &mut d);
        assert!(app.versions.is_some());
        // The whole column is the history's: a way back and its name over
        // the line on how to read it, then the versions.
        no_library(&mut d, &mut app);
        assert!(d.key_of("history-back").is_some(), "a way back");
        assert!(d.key_named("Back to the notes").is_some());
        assert!(drawn(&mut d, &mut app, "History"));
        assert!(d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.starts_with("Click one to read it."))));
        assert!(drawn(&mut d, &mut app, "The note as it is · +2 words"));
        assert!(drawn(&mut d, &mut app, "Where its history starts"));
        assert_eq!(keyed_rect(&mut d, "history-level").w, width, "the library's width");
        // The page shows the note as it is, with its way back.
        assert!(drawn(&mut d, &mut app, "Done"));

        match back {
            "escape" => key(&mut app, &mut d, "escape"),
            "the back button" => {
                let k = d.key_of("history-back").unwrap();
                d.click_key(&mut app, k);
            }
            _ => {
                let k = d.key_named("Done").unwrap();
                d.click_key(&mut app, k);
            }
        }
        d_frame_twice(&mut app, &mut d);
        assert!(app.versions.is_none(), "{back} closes it");
        // The library as it was: its folder, its search, where it was
        // scrolled.
        assert_eq!(app.folder.as_deref(), Some("Work"), "{back}");
        assert_eq!(app.query, "Note", "{back}");
        assert_eq!(d.core.edit_text(search).as_deref(), Some("Note"), "{back}: the search still reads it");
        assert_eq!(app.order.len(), count, "{back}");
        assert_eq!(d.key_of("library"), Some(list), "{back}");
        let now = d.core.scroll_offset(list).y;
        assert!((now - scrolled).abs() < 1.0, "{back}: scrolled where it was ({now}, was {scrolled})");
        assert!(d.key_of("history-level").is_none());
        assert!(drawn(&mut d, &mut app, "Work"), "the folders are back");
    }
    // The search's shortcut, or a new folder, is the library's: back to it.
    app.on_plain(Msg::ShowHistory);
    d_frame_twice(&mut app, &mut d);
    key_with(&mut app, &mut d, "f", CTRL);
    d.frame(&mut app);
    assert!(app.versions.is_none(), "searching closes the history");
    assert_eq!(d.core.focus(), Some(search), "and the search has the keyboard");
    app.on_plain(Msg::ShowHistory);
    app.on_plain(Msg::NewFolder);
    d_frame_twice(&mut app, &mut d);
    assert!(app.versions.is_none(), "so does a new folder");
    assert!(app.naming.is_some() && d.key_of("folders").is_some());
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    remove_test_dir(dir);
}

#[test]
fn the_arrows_walk_the_versions_and_enter_reads_one() {
    let (mut app, mut d, dir) = two_versions("history-keys");
    app.on_plain(Msg::ShowHistory);
    d_frame_twice(&mut app, &mut d);
    let row = |d: &mut Drive, name: &str| d.key_named(name).unwrap();
    // The keyboard is on the version shown, the note as it is.
    assert_eq!(d.core.focus(), Some(row(&mut d, "Edited here")));
    key(&mut app, &mut d, "down");
    assert_eq!(d.core.focus(), Some(row(&mut d, "Where its history starts")));
    assert_eq!(app.versions.as_ref().unwrap().shown, 0, "moved to, not read yet");
    assert_eq!(texts(&app)[0], "First words <mark style=\"background: #D8EAC0CC\">and more</mark>");
    key(&mut app, &mut d, "enter");
    assert_eq!(app.versions.as_ref().unwrap().shown, 1);
    assert_eq!(texts(&app)[0], "First words");
    key(&mut app, &mut d, "up");
    assert_eq!(d.core.focus(), Some(row(&mut d, "Edited here")));
    key(&mut app, &mut d, "enter");
    assert_eq!(app.versions.as_ref().unwrap().shown, 0);
    key(&mut app, &mut d, "escape");
    assert!(app.versions.is_none());
    assert!(d.key_of("library").is_some(), "the notes again");
    remove_test_dir(dir);
}

#[test]
fn on_a_narrow_window_the_history_is_where_the_list_would_be() {
    let (mut app, mut d, dir) = a_library_and_two_versions("history-narrow", 640.0);
    assert!(d.key_of("library").is_none(), "the note, not the list");
    app.on_plain(Msg::ShowHistory);
    d_frame_twice(&mut app, &mut d);
    // The whole window, as the list is.
    no_library(&mut d, &mut app);
    let level = keyed_rect(&mut d, "history-level");
    assert_eq!(level.w, 640.0);
    assert!(d.key_of("t-back").is_none(), "no page beside it");
    // A version read: the page, with a way back to the versions.
    let k = d.key_named("Where its history starts").unwrap();
    d.click_key(&mut app, k);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(texts(&app)[0], "First words");
    assert!(d.key_of("history-level").is_none());
    let back = d.key_of("t-back").expect("a way back");
    assert!(d.key_named("History").is_some(), "back to the history, it says");
    d.click_key(&mut app, back);
    d_frame_twice(&mut app, &mut d);
    assert!(d.key_of("history-level").is_some(), "the versions again");
    assert!(app.versions.is_some());
    key(&mut app, &mut d, "escape");
    d.frame(&mut app);
    assert!(app.versions.is_none());
    assert!(d.key_of("library").is_some(), "and the list of notes");
    remove_test_dir(dir);
}

/// The line over a version read: its words whole, never cut short, and
/// its actions on the same line where they fit, else together on the
/// next, and in the page's width at every window size.
#[test]
fn a_versions_line_wraps_whole_and_never_cuts_its_words() {
    for (w, one_line) in [(1500.0, true), (860.0, false), (640.0, false), (360.0, false)] {
        let (mut app, mut d, dir) = a_library_and_two_versions(&format!("history-line-{w}"), w);
        // A wide page has room for the whole line.
        if one_line {
            app.settings.width = crate::settings::Width::Wide;
        }
        app.on_plain(Msg::ShowHistory);
        app.on_plain(Msg::ShowVersion { index: 1 });
        d.core.set_inspect(true);
        d_frame_twice(&mut app, &mut d);
        d_frame_twice(&mut app, &mut d);
        let what = keyed_rect(&mut d, "version-what");
        let actions = keyed_rect(&mut d, "version-actions");
        let (restore, done) = (keyed_rect(&mut d, "restore"), keyed_rect(&mut d, "close-history"));
        let page = app.page_rect.expect("the page");
        let right = page.x + page.w;
        for (name, r) in [("words", what), ("actions", actions), ("Restore", restore), ("Done", done)] {
            assert!(r.x >= page.x && r.x + r.w <= right + 0.5, "{w}: {name} inside the page: {r:?} {page:?}");
        }
        // Its words are drawn whole: as wide as they are measured, or
        // wrapped onto more rows than one, never "…".
        let node = d.core.nodes().iter().find(|n| n.text.as_deref().is_some_and(|t| t.starts_with("The version from"))).cloned();
        let node = node.expect("the version's words");
        assert!(!node.text.as_deref().unwrap().contains('\u{2026}'), "{w}: not cut short");
        if one_line {
            assert!((restore.y + restore.h / 2.0 - (what.y + what.h / 2.0)).abs() < 2.0, "{w}: one line: {what:?} {restore:?}");
        } else {
            assert!(actions.y >= what.y + what.h - 0.5, "{w}: the actions under the words: {what:?} {actions:?}");
            assert!(actions.x <= what.x + 0.5, "{w}: from the line's start");
            // Together: Restore and Done on one row, where it fits.
            if w >= 640.0 {
                assert!((restore.y - done.y).abs() < 1.0, "{w}: the actions stay together: {restore:?} {done:?}");
            }
        }
        // The note stays under the line, whatever its height.
        let words = d.core.nodes().iter().find(|n| n.text.as_deref().is_some_and(|t| t.starts_with("First words"))).map(|n| n.rect);
        let words = words.expect("the version's text");
        assert!(words.y >= actions.y + actions.h, "{w}: the note below: {words:?} {actions:?}");
        let warnings = d.warnings();
        assert!(warnings.is_empty(), "{w}: {warnings:?}");
        remove_test_dir(dir);
    }
}

#[test]
fn the_history_shows_with_the_library_hidden_and_goes_with_it() {
    let (mut app, mut d, dir) = two_versions("history-hidden");
    app.on_settings(Msg::ToggleSidebar);
    assert!(!app.settings.sidebar);
    let settle = |app: &mut Carnet, d: &mut Drive| {
        for _ in 0..20 {
            d.advance(0.05);
            d.frame(app);
        }
    };
    settle(&mut app, &mut d);
    let pane = |d: &mut Drive| keyed_rect(d, "library-pane").w;
    assert_eq!(pane(&mut d), 0.0);
    app.on_plain(Msg::ShowHistory);
    settle(&mut app, &mut d);
    assert_eq!(pane(&mut d), app.library_w(), "the history shows where the library would");
    assert!(d.key_of("history-level").is_some());
    key(&mut app, &mut d, "escape");
    settle(&mut app, &mut d);
    assert_eq!(pane(&mut d), 0.0, "and goes again with it");
    assert!(!app.settings.sidebar, "the library stays hidden");
    // The column's switch puts the history away; with the library shown
    // it goes too.
    app.on_plain(Msg::ShowHistory);
    app.on_settings(Msg::ToggleSidebar);
    assert!(app.versions.is_none());
    assert!(!app.settings.sidebar);
    app.on_settings(Msg::ToggleSidebar);
    app.on_plain(Msg::ShowHistory);
    app.on_settings(Msg::ToggleSidebar);
    assert!(app.versions.is_none());
    assert!(!app.settings.sidebar, "hidden, as the switch says");
    remove_test_dir(dir);
}

#[test]
fn settings_sync_says_what_is_wrong_in_place() {
    let (mut app, mut d, dir) = fresh("sync-settings");
    app.settings_open = true;
    assert!(drawn(&mut d, &mut app, "Sync"));
    // The server first.
    assert!(drawn(&mut d, &mut app, "Cloud sync setup") && drawn(&mut d, &mut app, "Step 1 of 3"));
    assert!(d.key_of("sync-code").is_none(), "no way in before the server");
    app.sync_server = "notes.example.com".into();
    app.sync_code = "47-12".into();
    app.on_plain(Msg::SyncStart);
    d.frame(&mut app);
    assert!(app.sync.error.as_deref().is_some_and(|e| e.starts_with("That isn't a code")));
    assert!(!app.sync.is_on());
    // (The inspector shortens a long text: its start is enough.)
    let said = app.sync.error.clone().unwrap();
    let _ = drawn(&mut d, &mut app, "");
    assert!(d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|x| x.starts_with(&said[..40]))), "said in place");
    // A server that takes passwords: a code or them, chosen; the words
    // asked for apart. One field for a code, whichever kind.
    app.connect.assume("https://notes.example.com", server_info(&["passwords"]));
    assert!(drawn(&mut d, &mut app, "Enter the code") && drawn(&mut d, &mut app, "Email and password"));
    assert!(drawn(&mut d, &mut app, "Use recovery words"));
    assert!(!drawn(&mut d, &mut app, "Pairing code"), "no kinds of code to choose between");
    app.on_plain(Msg::SyncHow { to: 1 });
    d.frame(&mut app);
    assert!(drawn(&mut d, &mut app, "Recover") && drawn(&mut d, &mut app, "Step 3 of 3"));
    app.on_plain(Msg::SyncBack);
    // The third way: an email and password, the password as dots.
    assert!(drawn(&mut d, &mut app, "Email and password"));
    app.on_plain(Msg::SyncHow { to: 2 });
    d.frame(&mut app);
    assert!(drawn(&mut d, &mut app, "Sign in"));
    // Show is an eye at the password field's end, not a button beside it.
    assert!(!drawn(&mut d, &mut app, "Show"));
    let field = d.rect_of(app_field(&app, Field::SyncPassword).unwrap()).unwrap();
    let eye = d.key_of("sync-password-eye").expect("the eye");
    assert_eq!(d.key_named("Show password"), Some(eye), "named for what it does");
    let r = d.rect_of(eye).unwrap();
    assert!(r.x > field.x + field.w && r.x + r.w < field.x + field.w + 40.0, "{field:?} {r:?}");
    app.on_plain(Msg::SyncShowPassword);
    d_frame_twice(&mut app, &mut d);
    assert!(app.sync_show_password && d.key_named("Hide password") == Some(eye));
    app.sync_email = "not an email".into();
    app.sync_password = "secret".into();
    app.on_plain(Msg::SyncStart);
    assert_eq!(app.sync.error.as_deref(), Some("That doesn't look like an email address"));
    remove_test_dir(dir);
}

/// The password field is kui's own: read out as a password, never its
/// letters, and shown as the plain words it holds once the eye is open.
#[test]
fn the_password_field_is_a_password_field_until_shown() {
    let (store, dir) = temp_store("pw-secure");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 1600.0).framing();
    app.settings_open = true;
    app.connect.assume("https://notes.example.com", server_info(&["passwords"]));
    app.sync_password = "hunter22".into();
    app.on_plain(Msg::SyncHow { to: 2 });
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    let fields = |d: &mut Drive| d.core.access_tree().nodes.iter().filter(|n| n.role == kui_native::Role::PasswordInput).map(|n| n.name.clone()).collect::<Vec<_>>();
    assert_eq!(fields(&mut d), [Some("Password".to_string())], "one password field, by its name");
    assert!(!d.core.access_tree().nodes.iter().any(|n| n.name.as_deref().is_some_and(|t| t.contains("hunter22"))), "its letters are never read out");
    app.on_plain(Msg::SyncShowPassword);
    d_frame_twice(&mut app, &mut d);
    assert!(fields(&mut d).is_empty(), "shown: a plain field");
    assert_eq!(app.sync_password, "hunter22", "showing it changes nothing");
    let _ = std::fs::remove_dir_all(dir);
}

/// A new device comes in by an email and password set on another, through
/// a real server that takes them: with the notes. A wrong password lets
/// nothing in, and says so.
#[test]
fn a_new_device_comes_in_by_email_and_password() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-password-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    // Someone's own account (a hosted signup's, not the server's owner's).
    let invite = server::invite(&mut carnet_server::Sharded::open(&data).unwrap(), false).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let host = server::Host { passwords: true, ..server::Host::default() };
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, host, async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("pw-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut a, &mut da, &|x| x.sync.status == crate::sync::Status::Online);
    type_in(&mut a, &mut da, "Packing list");
    key(&mut a, &mut da, "enter");
    type_in(&mut a, &mut da, "passport, charger");
    a.save_current();
    a.settings_open = true;
    until("A knows the server takes passwords", &mut a, &mut da, &|x| x.sync.password == Some((true, None)));
    assert!(drawn(&mut da, &mut a, "Set one"));
    a.on_plain(Msg::SyncPassword);
    // One letter short of what the library takes: Save stays off. Asked
    // anyway, why is said by the password's field, above its Save, not
    // under the whole card; an edit takes it away.
    let short = "a".repeat(carnet_sync::password::MIN_LEN - 1);
    a.pw_edit = Some(("Ann@Example.com".into(), short.clone()));
    let saves = |d: &mut Drive, app: &mut Carnet| {
        d.core.set_inspect(true);
        d.frame(app);
        d.core.nodes().iter().filter(|n| n.label.as_deref() == Some("pw-save") && !n.flags.contains(&"disabled")).count()
    };
    assert_eq!(saves(&mut da, &mut a), 0, "{} letters", short.len());
    a.on_plain(Msg::SyncPasswordSave);
    let why = a.sync.password_error.clone().expect("said why");
    assert!(why.contains(&format!("{} letters", carnet_sync::password::MIN_LEN)), "{why}");
    assert_eq!(a.sync.error, None, "not the card's");
    assert!(drawn(&mut da, &mut a, &why));
    let nodes = da.core.nodes();
    let said_at = nodes.iter().find(|n| n.text.as_deref() == Some(why.as_str())).unwrap().rect.y;
    let save_at = nodes.iter().find(|n| n.label.as_deref() == Some("pw-save")).unwrap().rect.y;
    let field_at = da.rect_of(app_field(&a, Field::PasswordNew).unwrap()).unwrap().y;
    assert!(field_at < said_at && said_at < save_at, "field {field_at}, said {said_at}, Save {save_at}");
    a.on_field_changed(Field::PasswordNew, format!("{short}b"));
    assert_eq!(a.sync.password_error, None, "edited: gone");
    assert!(!drawn(&mut da, &mut a, &why));
    assert_eq!(saves(&mut da, &mut a), 1, "long enough: Save");
    // Said again, then a password that's taken: gone for good.
    a.pw_edit = Some(("Ann@Example.com".into(), short.clone()));
    a.on_plain(Msg::SyncPasswordSave);
    assert!(a.sync.password_error.is_some());
    // Typed in the field, as a hand would.
    a.pw_edit = Some(("Ann@Example.com".into(), String::new()));
    da.frame(&mut a);
    let pw_field = app_field(&a, Field::PasswordNew).unwrap();
    da.core.set_edit_text(pw_field, "");
    da.focus(&mut a, pw_field);
    da.text(&mut a, "correct horse battery");
    assert_eq!(a.pw_edit.as_ref().map(|p| p.1.as_str()), Some("correct horse battery"));
    a.on_plain(Msg::SyncPasswordSave);
    until("set", &mut a, &mut da, &|x| x.notice.as_ref().is_some_and(|n| n.text.starts_with("Password set")));
    assert_eq!((a.sync.password_error.as_deref(), a.sync.error.as_deref()), (None, None));
    assert!(!drawn(&mut da, &mut a, &why));
    until("listed", &mut a, &mut da, &|x| x.sync.password == Some((true, Some("ann@example.com".into()))));
    // Set, the field put away keeps nothing of it; Change opens it empty.
    da.frame(&mut a);
    assert!(a.pw_edit.is_none());
    // (kui forgets a password field the frame it isn't drawn.)
    assert!(da.core.edit_text(pw_field).is_none_or(|t| t.is_empty()), "nothing kept in the field put away");
    assert!(drawn(&mut da, &mut a, "Change"));
    a.on_plain(Msg::SyncPassword);
    d_frame_twice(&mut a, &mut da);
    let k = app_field(&a, Field::PasswordNew).unwrap();
    assert_eq!(da.core.edit_text(k).as_deref(), Some(""), "Change opens empty");
    assert_eq!(a.pw_edit.as_ref().map(|p| p.1.as_str()), Some(""));
    // Typed and left, Settings closed: forgotten, by the app and the field.
    da.focus(&mut a, k);
    da.text(&mut a, "left behind here");
    assert_eq!(a.pw_edit.as_ref().map(|p| p.1.as_str()), Some("left behind here"));
    a.settings_open = false;
    d_frame_twice(&mut a, &mut da);
    a.settings_open = true;
    d_frame_twice(&mut a, &mut da);
    assert_eq!(a.pw_edit.as_ref().map(|p| p.1.as_str()), Some(""), "Settings closed: forgotten");
    assert_eq!(da.core.edit_text(app_field(&a, Field::PasswordNew).unwrap()).as_deref(), Some(""));
    a.on_plain(Msg::SyncPasswordCancel);

    // A wrong one: nothing let in, and said.
    let (mut c, mut dc, dir_c) = fresh("pw-c");
    c.sync_server = origin.clone();
    c.sync_how = 2;
    c.sync_email = "ann@example.com".into();
    c.sync_password = "wrong horse battery".into();
    c.on_plain(Msg::SyncStart);
    until("C refused", &mut c, &mut dc, &|x| x.sync.error.is_some());
    assert!(c.sync.error.as_deref().is_some_and(|e| e.contains("don't match")), "{:?}", c.sync.error);
    assert!(!c.sync.is_on());

    let (mut b, mut db, dir_b) = fresh("pw-b");
    b.sync_server = origin.clone();
    b.sync_how = 2;
    b.sync_email = "ANN@example.com".into();
    b.sync_password = "correct horse battery".into();
    b.on_plain(Msg::SyncStart);
    let t0 = Instant::now();
    while !b.store.notes.iter().any(|n| n.title == "Packing list") {
        assert!(t0.elapsed() < Duration::from_secs(20), "timed out: B has the note");
        da.frame(&mut a);
        db.frame(&mut b);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(b.sync.is_on());
    assert!(b.sync_password.is_empty(), "the password signed in with isn't kept");

    // Deleted from A: asked first, then gone everywhere; each says so.
    a.settings_open = true;
    da.frame(&mut a);
    assert!(drawn(&mut da, &mut a, "Delete the account"));
    a.on_plain(Msg::SyncDeleteAccount);
    assert!(drawn(&mut da, &mut a, "Yes, delete the account"));
    a.on_plain(Msg::SyncDeleteAccount);
    let t0 = Instant::now();
    while a.sync.is_on() || b.sync.is_on() {
        assert!(t0.elapsed() < Duration::from_secs(20), "timed out: both let go");
        da.frame(&mut a);
        db.frame(&mut b);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(a.sync.error.as_deref().is_some_and(|e| e.starts_with("The account is deleted")), "{:?}", a.sync.error);
    assert!(b.sync.error.as_deref().is_some_and(|e| e.contains("isn't on the server any more")), "{:?}", b.sync.error);
    assert!(a.store.notes.iter().any(|n| n.title == "Packing list"), "the notes stay");

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    for dir in [dir_a, dir_b, dir_c] {
        remove_test_dir(dir);
    }
    let _ = std::fs::remove_dir_all(&data);
}

/// The server gone: Settings → Sync says "Offline", and why in plain
/// words with an icon; what the network said ("Connection refused (os
/// error 61)") only on hover.
#[test]
fn offline_is_said_in_plain_words() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-offline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let (mut a, mut da, dir_a) = fresh("offline");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("online", &mut a, &mut da, &|a| a.sync.status == crate::sync::Status::Online);
    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    drop(rt);
    until("offline", &mut a, &mut da, &|a| matches!(a.sync.status, crate::sync::Status::Offline(_)));
    a.settings_open = true;
    assert!(drawn(&mut da, &mut a, "Offline"));
    assert!(drawn(&mut da, &mut a, "Can't reach the server. Trying again\u{2026}"), "{:?}", a.sync.status);
    let jargon = da.core.nodes().iter().filter_map(|n| n.text.clone()).find(|t| t.contains("error") || t.contains("refused") || t.contains("closed"));
    assert_eq!(jargon, None, "the network's own words on the page");
    assert!(a.sync.offline_detail.is_some(), "kept for the tooltip");
    assert_eq!(crate::sync::offline_words("IO error: Connection refused (os error 61)"), "Can't reach the server. Trying again\u{2026}");
    assert_eq!(crate::sync::offline_words("timed out connecting"), "Can't reach the server. Trying again\u{2026}");
    assert!(crate::sync::offline_words("invalid peer certificate: UnknownIssuer").contains("certificate"));
    remove_test_dir(dir_a);
    let _ = std::fs::remove_dir_all(&data);
}

/// Another device's edit to the open note leaves the caret where it was
/// in the words: on the line it was on, after the same letters, whether
/// the change was in another block, before it in its own, or a block put
/// in above; with edits here not yet saved (a merge) too. The next letter
/// typed lands there, not on the first line.
#[test]
fn a_remote_edit_leaves_the_caret_where_it_was() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-caret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    #[allow(clippy::type_complexity)]
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&[(&mut Carnet, &mut Drive)]) -> bool| {
        let t0 = Instant::now();
        while !done(apps) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            for (app, d) in apps.iter_mut() {
                d.frame(&mut **app);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    // Block `i`'s words, and where the caret is: (block, byte).
    let words = |app: &Carnet, i: usize| app.doc().blocks().get(i).and_then(|b| b.text_ref()).unwrap_or("").to_string();
    let caret = |app: &Carnet| (app.doc().index_of(app.caret.head.slot.block).unwrap(), app.caret.head.byte);

    let (mut a, mut da, dir_a) = fresh("caret-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    type_in(&mut a, &mut da, "Shopping");
    key(&mut a, &mut da, "enter");
    type_in(&mut a, &mut da, "- milk1");
    key(&mut a, &mut da, "enter");
    type_in(&mut a, &mut da, "bread1");
    a.save_current();
    a.on_plain(Msg::SyncPair);
    until("a code", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.code.is_some());
    let (mut b, mut db, dir_b) = fresh("caret-b");
    b.sync_server = origin.clone();
    b.sync_code = a.sync.code.clone().unwrap();
    b.on_plain(Msg::SyncStart);
    until("B has the note", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| x[1].0.store.notes.iter().any(|n| n.title == "Shopping"));
    let i = b.store.notes.iter().position(|n| n.title == "Shopping").unwrap();
    b.open(i);
    d_frame_twice(&mut b, &mut db);
    a.settings_open = false;
    d_frame_twice(&mut a, &mut da);
    assert_eq!((words(&a, 0), words(&a, 1)), ("milk1".to_string(), "bread1".to_string()));
    assert_eq!(caret(&a), (1, 6), "A's caret at the end of the second item");
    // B: an edit there, saved, sent; A waits for it.
    let edit = |b: &mut Carnet, db: &mut Drive, f: &dyn Fn(&mut Doc)| {
        let note = &mut b.store.notes[b.current];
        f(&mut note.doc);
        note.touch();
        b.save_current();
        db.frame(b);
    };

    // In another block: " WEB" at the end of the first item.
    edit(&mut b, &mut db, &|d| d.block_mut(0).text_mut().unwrap().push_str(" WEB"));
    until("A has B's WEB", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| words(x[0].0, 0) == "milk1 WEB");
    assert_eq!(caret(&a), (1, 6), "still at the end of the second item");
    key(&mut a, &mut da, "x");
    assert_eq!((words(&a, 0), words(&a, 1)), ("milk1 WEB".to_string(), "bread1x".to_string()), "typed where it was");
    a.save_current();
    until("B has A's x", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| words(x[1].0, 1) == "bread1x");

    // In its own block, before it: the caret moves with its words.
    edit(&mut b, &mut db, &|d| d.block_mut(1).text_mut().unwrap().insert_str(0, "fresh "));
    until("A has B's fresh", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| words(x[0].0, 1) == "fresh bread1x");
    assert_eq!(caret(&a), (1, 13), "after bread1x still");
    // A block put in above: the same line, the same place in it.
    edit(&mut b, &mut db, &|d| d.insert(0, Block::text("At noon")));
    until("A has B's line", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| words(x[0].0, 0) == "At noon");
    assert_eq!(caret(&a), (2, 13), "the item, one block down now");
    key(&mut a, &mut da, "y");
    assert_eq!(words(&a, 2), "fresh bread1xy");

    // Unsaved here, a change there: merged, and the caret kept too.
    key(&mut a, &mut da, "z");
    edit(&mut b, &mut db, &|d| d.block_mut(1).text_mut().unwrap().push_str(" NOW"));
    until("A has B's NOW", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| words(x[0].0, 1) == "milk1 WEB NOW");
    assert_eq!(words(&a, 2), "fresh bread1xyz");
    assert_eq!(caret(&a), (2, 15), "after the z typed here");
    assert!(a.caret.is_empty());

    // The title changed there: the title shown here follows, the caret
    // where it was.
    a.save_current();
    let note = &mut b.store.notes[b.current];
    note.title = "Shopping list".into();
    note.touch();
    b.save_current();
    db.frame(&mut b);
    until("A has B's title", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| x[0].0.store.notes[x[0].0.current].title == "Shopping list");
    d_frame_twice(&mut a, &mut da);
    let title = da.key_of("title").expect("the title field");
    assert_eq!(da.core.edit_text(title).as_deref(), Some("Shopping list"), "the field shows it");
    assert_eq!(caret(&a), (2, 15), "the caret where it was");

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir_a);
    remove_test_dir(dir_b);
    let _ = std::fs::remove_dir_all(&data);
}

/// A device nobody looks at (its window covered, minimized, hidden)
/// draws no frames of its own: the app runs only when something wakes
/// it. Sync's socket and its pictures' downloads wake it, so another
/// device's edit reaches this one's file on disk with no frame other than
/// those wakes, and no timer, asked for.
#[test]
fn another_devices_edit_lands_with_only_wakes_to_run_on() {
    use carnet_server::server::{self, Config};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-woken-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    #[allow(clippy::type_complexity)]
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&[(&mut Carnet, &mut Drive)]) -> bool| {
        let t0 = Instant::now();
        while !done(apps) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            for (app, d) in apps.iter_mut() {
                d.frame(&mut **app);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("woken-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    type_in(&mut a, &mut da, "Errands");
    key(&mut a, &mut da, "enter");
    type_in(&mut a, &mut da, "post office");
    a.save_current();
    a.on_plain(Msg::SyncPair);
    until("a code", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.code.is_some());
    // B's wakes counted: what the shell's waker would turn into frames.
    let (mut b, mut db, dir_b) = fresh("woken-b");
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    b.sync.set_wake(move || {
        w.fetch_add(1, Ordering::Relaxed);
    });
    b.sync_server = origin.clone();
    b.sync_code = a.sync.code.clone().unwrap();
    b.on_plain(Msg::SyncStart);
    until("B has the note", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| x[1].0.store.notes.iter().any(|n| n.title == "Errands"));
    a.settings_open = false;
    // Settled: B's last frames done, its wakes since then what count.
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(500) {
        da.frame(&mut a);
        db.frame(&mut b);
        std::thread::sleep(Duration::from_millis(5));
    }
    let path = b.store.notes.iter().find(|n| n.title == "Errands").and_then(|n| n.path.clone()).expect("B's file");

    // Each edit on A; B framed only when woken.
    for (round, words) in ["the bank", "the library", "the pharmacy"].into_iter().enumerate() {
        let i = a.store.notes.iter().position(|n| n.title == "Errands").unwrap();
        a.open(i);
        da.frame(&mut a);
        let note = &mut a.store.notes[a.current];
        note.doc.insert(1, Block::text(words));
        note.touch();
        a.save_current();
        let mut seen = wakes.load(Ordering::Relaxed);
        let (mut frames, t0) = (0, Instant::now());
        while !std::fs::read_to_string(&path).is_ok_and(|t| t.contains(words)) {
            assert!(t0.elapsed() < Duration::from_secs(10), "round {round}: B's file never had \"{words}\" ({frames} woken frames)");
            da.frame(&mut a);
            let now = wakes.load(Ordering::Relaxed);
            if now != seen {
                seen = now;
                frames += 1;
                db.frame(&mut b);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(frames > 0, "round {round}: woken");
        assert!(b.store.notes.iter().any(|n| n.markdown().contains(words)), "round {round}: and in B's list");
    }

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir_a);
    remove_test_dir(dir_b);
    let _ = std::fs::remove_dir_all(&data);
}

/// A minimized window draws nothing, and on Windows the platform gives it
/// no frame at all: kui builds its frame anyway on a wake and at a time
/// its view asked for, and draws nothing until it is back. So another
/// device's edits reach this one's file and its list while it sits on
/// the taskbar, with nothing drawn, and the window brought back shows
/// them, at the size it had: a minimized window is no size on Windows,
/// and none reaches the app.
#[test]
fn another_devices_edit_lands_in_a_minimized_window() {
    use carnet_server::server::{self, Config};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-minimized-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    #[allow(clippy::type_complexity)]
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&[(&mut Carnet, &mut Drive)]) -> bool| {
        let t0 = Instant::now();
        while !done(apps) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            for (app, d) in apps.iter_mut() {
                d.frame(&mut **app);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("minimized-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    type_in(&mut a, &mut da, "Errands");
    key(&mut a, &mut da, "enter");
    type_in(&mut a, &mut da, "post office");
    a.save_current();
    a.on_plain(Msg::SyncPair);
    until("a code", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.code.is_some());
    let (mut b, mut db, dir_b) = fresh("minimized-b");
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    b.sync.set_wake(move || {
        w.fetch_add(1, Ordering::Relaxed);
    });
    b.sync_server = origin.clone();
    b.sync_code = a.sync.code.clone().unwrap();
    b.on_plain(Msg::SyncStart);
    until("B has the note", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| x[1].0.store.notes.iter().any(|n| n.title == "Errands"));
    a.settings_open = false;
    let i = b.store.notes.iter().position(|n| n.title == "Errands").unwrap();
    b.open(i);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(500) {
        da.frame(&mut a);
        db.frame(&mut b);
        std::thread::sleep(Duration::from_millis(5));
    }
    let path = b.store.notes[b.current].path.clone().expect("B's file");

    // Minimized: on Windows the window reports no size.
    db.hide();
    db.viewport = kui_native::Size::new(0.0, 0.0);
    let drawn = db.drawn();
    for (round, words) in ["the bank", "the library"].into_iter().enumerate() {
        let i = a.store.notes.iter().position(|n| n.title == "Errands").unwrap();
        a.open(i);
        da.frame(&mut a);
        let note = &mut a.store.notes[a.current];
        note.doc.insert(1, Block::text(words));
        note.touch();
        a.save_current();
        let mut seen = wakes.load(Ordering::Relaxed);
        let (mut woken, t0) = (0, Instant::now());
        while !std::fs::read_to_string(&path).is_ok_and(|t| t.contains(words)) {
            assert!(t0.elapsed() < Duration::from_secs(10), "round {round}: B's file never had \"{words}\" ({woken} wakes)");
            da.frame(&mut a);
            // What the runner does for a window it cannot draw: a frame
            // on a wake, and at a time the view asked for.
            let now = wakes.load(Ordering::Relaxed);
            if now != seen {
                seen = now;
                woken += 1;
                db.wake(&mut b);
            }
            db.advance(0.005);
            db.due(&mut b);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(woken > 0, "round {round}: woken");
        assert!(b.store.notes[b.current].markdown().contains(words), "round {round}: the open note has it");
    }
    assert_eq!(db.drawn(), drawn, "nothing drawn while minimized");

    // Restored, at the size it had: the note drawn with both.
    db.viewport = kui_native::Size::new(1100.0, 760.0);
    db.show(&mut b);
    assert_eq!(db.drawn(), drawn + 1);
    let shown: Vec<String> = db.core.nodes().iter().filter_map(|n| n.text.clone()).collect();
    for words in ["the bank", "the library"] {
        assert!(shown.iter().any(|t| t.contains(words)), "{words} drawn: {shown:?}");
    }
    let w = db.warnings();
    assert!(w.is_empty(), "{w:?}");

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir_a);
    remove_test_dir(dir_b);
    let _ = std::fs::remove_dir_all(&data);
}

/// A recovery code's words show once the server says it keeps them, not
/// before; making another, when there is one, is asked first, as the
/// words written down stop working. And keys kept in a file, not a
/// keychain (Linux with none running), are said so in Settings.
#[test]
fn the_recovery_code_is_replaced_only_when_asked() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    /// Secrets as a keychain-less Linux keeps them: in a file.
    #[derive(Default)]
    struct InFile(crate::sync::InMemory);
    impl crate::sync::Secrets for InFile {
        fn get(&self, name: &str) -> Option<Vec<u8>> {
            self.0.get(name)
        }
        fn set(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
            self.0.set(name, bytes)
        }
        fn delete(&self, name: &str) {
            self.0.delete(name)
        }
        fn in_file(&self) -> bool {
            true
        }
    }
    let data = std::env::temp_dir().join(format!("carnet-app-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::invite(&mut carnet_server::Sharded::open(&data).unwrap(), false).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, server::Host::default(), async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("recovery-a");
    a.sync = crate::sync::Sync::load(dir_a.join("no-sync.txt"), std::sync::Arc::new(InFile::default()));
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("online", &mut a, &mut da, &|x| x.sync.status == crate::sync::Status::Online);
    a.settings_open = true;
    until("the devices listed", &mut a, &mut da, &|x| x.sync.devices.is_some());
    assert!(drawn(&mut da, &mut a, crate::sync::KEYS_IN_FILE), "keys in a file, said so");

    // None yet: made at once, its words shown only once the server keeps it.
    assert!(drawn(&mut da, &mut a, "Make one"));
    assert!(drawn(&mut da, &mut a, "Twelve words that let a new device in when none of yours is left."));
    a.on_plain(Msg::SyncRecovery);
    assert!(!a.sync_words_armed && a.sync.words.is_none(), "not shown before the server says");
    assert!(drawn(&mut da, &mut a, "Making one\u{2026}"));
    until("the words", &mut a, &mut da, &|x| x.sync.words.is_some());
    let first = a.sync.words.clone().unwrap();
    a.on_plain(Msg::SyncRecoveryDone);
    until("the code listed", &mut a, &mut da, &|x| x.sync.has_recovery());

    // One there: the row says so, and its button replaces it.
    assert!(drawn(&mut da, &mut a, "Replace"));
    assert!(!drawn(&mut da, &mut a, "Make one"));
    assert!(drawn(&mut da, &mut a, "This account has one: twelve words that let a new device in when none of yours is left."));

    // One there: asked first; Cancel makes none.
    a.on_plain(Msg::SyncRecovery);
    assert!(a.sync_words_armed);
    assert!(drawn(&mut da, &mut a, "Replace the recovery code? The words you have now will stop working."));
    assert!(drawn(&mut da, &mut a, "Yes, replace"));
    a.on_plain(Msg::SyncRecoveryCancel);
    assert!(!a.sync_words_armed && !a.sync.making_recovery());
    // Yes, replace: new words, once kept.
    a.on_plain(Msg::SyncRecovery);
    a.on_plain(Msg::SyncRecovery);
    assert!(!a.sync_words_armed && a.sync.making_recovery());
    until("the new words", &mut a, &mut da, &|x| x.sync.words.is_some());
    assert_ne!(a.sync.words.as_deref(), Some(first.as_str()));

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir_a);
    let _ = std::fs::remove_dir_all(&data);
}

/// On a hosted service that holds passwords to the email signed up with,
/// Settings shows that email instead of a field, Save sets it with that
/// one, and another (asked by hand) is refused in the service's words.
#[test]
fn a_hosted_password_signs_in_with_the_email_signed_up_with() {
    use carnet_server::quota::{Admission, PasswordEmails};
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    struct Signup;
    impl Admission for Signup {
        fn admitted(&self, _: [u8; 32], _: &[u8]) {}
        fn email_of(&self, _: &[u8]) -> Option<String> {
            Some("ann@example.com".into())
        }
    }
    let data = std::env::temp_dir().join(format!("carnet-app-pwemail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::invite(&mut carnet_server::Sharded::open(&data).unwrap(), false).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let host = server::Host { passwords: true, password_emails: PasswordEmails::Account, admission: std::sync::Arc::new(Signup), first_invite: false, ..server::Host::default() };
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, host, async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("pwemail-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut a, &mut da, &|x| x.sync.status == crate::sync::Status::Online);
    a.settings_open = true;
    until("A knows the email", &mut a, &mut da, &|x| x.sync.password_email.as_deref() == Some("ann@example.com"));
    assert!(drawn(&mut da, &mut a, "Set one"));

    // Another email, asked by hand: the service's own words.
    a.sync.set_password("bob@example.com", "correct horse battery");
    until("refused", &mut a, &mut da, &|x| x.sync.error.is_some());
    assert!(a.sync.error.as_deref().is_some_and(|e| e.contains("the email you signed up with: ann@example.com")), "{:?}", a.sync.error);

    // Set one: the email shown, not asked; the password typed.
    a.on_plain(Msg::SyncPassword);
    assert_eq!(a.pw_edit.as_ref().map(|(e, _)| e.as_str()), Some("ann@example.com"));
    assert!(drawn(&mut da, &mut a, "ann@example.com"), "the email shown, not a field");
    // (The inspector keeps a line's first 60 letters.)
    assert!(da.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.starts_with("On this service, the password signs in with the email"))));
    assert_eq!(a.focus_req, None, "the password's field has the focus");
    a.pw_edit = Some(("someone@else.com".into(), "correct horse battery".into()));
    a.on_plain(Msg::SyncPasswordSave);
    until("set", &mut a, &mut da, &|x| x.notice.as_ref().is_some_and(|n| n.text.starts_with("Password set")));
    until("listed", &mut a, &mut da, &|x| x.sync.password == Some((true, Some("ann@example.com".into()))));

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir_a);
    let _ = std::fs::remove_dir_all(&data);
}

/// New here, on a server that takes signups (a hosted one; its signup
/// route faked, the invite it would mail made beforehand): Settings → Sync
/// asks whether it does, offers "New here?", asks for an invite for the
/// email typed (JSON, no Origin: as the app sends it), says where it went
/// with the code field focused, and the code pasted there makes the
/// account. A refusal is said in the server's words. A server of one's
/// own offers none of it.
#[test]
fn new_here_an_invite_asked_for_in_settings_makes_the_account() {
    use axum::http::{HeaderMap, StatusCode, header};
    use carnet_server::server::{self, Config};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-signup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::invite(&mut carnet_server::Sharded::open(&data).unwrap(), false).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    // Each signup asked for: its type, its Origin, its body. The first is
    // refused, as a service mailing that address lately might.
    type Asked = (Option<String>, Option<String>, String);
    let asked: Arc<Mutex<Vec<Asked>>> = Arc::default();
    let log = asked.clone();
    let signup = move |headers: HeaderMap, body: String| {
        let log = log.clone();
        async move {
            let h = |n| headers.get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
            let mut log = log.lock().unwrap();
            log.push((h(header::CONTENT_TYPE), h(header::ORIGIN), body));
            let json = [(header::CONTENT_TYPE, "application/json")];
            match log.len() {
                1 => (StatusCode::TOO_MANY_REQUESTS, json, r#"{"error":"Mailed lately: look for it, or wait ten minutes."}"#),
                2 => (StatusCode::ACCEPTED, json, r#"{"sent":true}"#),
                // Asked again within the mail limit: nothing mailed.
                _ => (StatusCode::ACCEPTED, json, r#"{"sent":false,"mailed_ago":40,"again_in":560}"#),
            }
        }
    };
    let host = server::Host {
        routes: axum::Router::new().route("/v1/signup", axum::routing::post(signup)),
        signup: Some(format!("{origin}/start")),
        first_invite: false,
        ..server::Host::default()
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, host, async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let focused = |app: &Carnet, d: &Drive| d.core.focus().and_then(|k| app.fields.get(&k)).copied();

    let (store, dir) = temp_store("signup");
    let mut a = Carnet::new(store);
    // Tall enough that Settings → Sync shows whole, to click in.
    let mut d = Drive::new(Core::new(), 1100.0, 4400.0).framing();
    a.settings_open = true;
    d_frame_twice(&mut a, &mut d);
    // The server typed, and Enter: asked what it is.
    let server = app_field(&a, Field::SyncServer).expect("the server's field");
    d.click_key(&mut a, server);
    d.frame(&mut a);
    type_in(&mut a, &mut d, &origin);
    assert_eq!(a.sync_server, origin);
    key(&mut a, &mut d, "enter");
    assert!(a.connect.checking() || a.connect.step() == crate::connect::Step::Way);
    until("checked", &mut a, &mut d, &|x| x.connect.step() == crate::connect::Step::Way);
    assert!(drawn(&mut d, &mut a, "Carnet server, up to date") && drawn(&mut d, &mut a, "Step 2 of 3"));
    assert!(drawn(&mut d, &mut a, "Enter the code"));
    assert!(!drawn(&mut d, &mut a, "Email and password"), "this one takes no passwords");
    let open = d.key_of("sync-way-3").expect("Make an account");
    assert_eq!(d.key_named("Make an account"), Some(open), "the row is a button, by its name");
    let r = d.rect_of(open).unwrap();
    d.click(&mut a, r.x + r.w / 2.0, r.y + r.h / 2.0);
    d_frame_twice(&mut a, &mut d);
    assert!(drawn(&mut d, &mut a, "Make an account") && drawn(&mut d, &mut a, "Step 3 of 3"));
    // Only that: the code and the other ways in are out of the way.
    assert!(d.key_of("sync-code").is_none() && d.key_of("sync-way-1").is_none(), "no code field, no other ways in");
    assert!(drawn(&mut d, &mut a, "Back"));
    assert_eq!(focused(&a, &d), Some(Field::SignupEmail), "the email field has the keyboard");
    type_in(&mut a, &mut d, "ann@example.com");
    assert_eq!(a.signup.email, "ann@example.com");
    // Refused: in the server's words, said as its own, under the field.
    key(&mut a, &mut d, "enter");
    until("refused", &mut a, &mut d, &|x| x.signup.error.is_some());
    assert!(drawn(&mut d, &mut a, "The server says: Mailed lately: look for it, or wait ten minutes."));
    assert!(a.signup.sent_to.is_none());
    // Asked again: sent, and the code field has the keyboard.
    let send = d.key_of("signup-send").expect("Send me an invite");
    let r = d.rect_of(send).unwrap();
    d.click(&mut a, r.x + r.w / 2.0, r.y + r.h / 2.0);
    until("sent", &mut a, &mut d, &|x| x.signup.sent_to.is_some());
    d_frame_twice(&mut a, &mut d);
    assert!(drawn(&mut d, &mut a, "We sent an invite to ann@example.com. Paste its code here."));
    assert!(!drawn(&mut d, &mut a, "Send me an invite"), "the email field gone");
    assert!(drawn(&mut d, &mut a, "Enter the code") && drawn(&mut d, &mut a, "No invite yet?"), "the code's form, asked again if it doesn't come");
    assert_eq!(focused(&a, &d), Some(Field::SyncCode));
    {
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        for (kind, from, body) in asked.iter() {
            assert_eq!((kind.as_deref(), from.as_deref(), body.as_str()), (Some("application/json"), None, r#"{"email":"ann@example.com"}"#));
        }
    }
    // Asked again within the server's mail limit: nothing went, and it
    // says when the last did, and when another can be asked for.
    let again = d.key_of("signup-again").expect("Ask again");
    let r = d.rect_of(again).unwrap();
    d.click(&mut a, r.x + r.w / 2.0, r.y + r.h / 2.0);
    d_frame_twice(&mut a, &mut d);
    assert_eq!(a.signup.email, "ann@example.com", "the email as typed");
    let send = d.key_of("signup-send").expect("Send me an invite");
    let r = d.rect_of(send).unwrap();
    d.click(&mut a, r.x + r.w / 2.0, r.y + r.h / 2.0);
    until("answered again", &mut a, &mut d, &|x| x.signup.sent.as_ref().is_some_and(carnet_sync::signup::Sent::lately));
    d_frame_twice(&mut a, &mut d);
    assert!(drawn(
        &mut d,
        &mut a,
        "We emailed ann@example.com a moment ago. Look for it in your inbox (and the spam folder), and paste its code here. You can ask for another email in 10 minutes."
    ));
    assert!(!drawn(&mut d, &mut a, "We sent an invite to ann@example.com. Paste its code here."), "nothing sent, so not said sent");
    assert_eq!(focused(&a, &d), Some(Field::SyncCode));
    // The code pasted: in.
    type_in(&mut a, &mut d, &invite.text());
    assert_eq!(a.sync_code, invite.text());
    key(&mut a, &mut d, "enter");
    until("in", &mut a, &mut d, &|x| x.sync.status == crate::sync::Status::Online);
    assert_eq!(a.connect.step(), crate::connect::Step::Server, "the steps start again for next time");

    // A server of one's own: no signups, so nothing of them.
    let own = std::env::temp_dir().join(format!("carnet-app-signup-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&own);
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let own_origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop_own, stopped_own) = tokio::sync::oneshot::channel::<()>();
    let served_own = rt.spawn(server::serve(Config { data: own.clone(), origin: own_origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped_own.await;
    }));
    let (mut b, mut db, dir_b) = fresh("signup-own");
    b.settings_open = true;
    b.sync_server = own_origin;
    b.on_plain(Msg::SyncCheck);
    until("checked", &mut b, &mut db, &|x| !x.connect.checking());
    assert!(!drawn(&mut db, &mut b, "Make an account") && !drawn(&mut db, &mut b, "No invite yet?"));
    b.on_plain(Msg::SignupSend);
    assert!(!b.signup.sending(), "no invite asked of a server that makes no accounts");

    let _ = stop.send(());
    let _ = stop_own.send(());
    rt.block_on(served).unwrap().unwrap();
    rt.block_on(served_own).unwrap().unwrap();
    for dir in [dir, dir_b] {
        remove_test_dir(dir);
    }
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&own);
}

/// Waits, frame by frame, for `done`.
fn until_done(what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool) {
    let t0 = std::time::Instant::now();
    while !done(app) {
        assert!(t0.elapsed() < std::time::Duration::from_secs(20), "timed out: {what}");
        d.frame(app);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A fresh app, tall enough that Settings → Sync shows whole, to click
/// in; Settings open.
fn tall(name: &str) -> (Carnet, Drive, std::path::PathBuf) {
    let (store, dir) = temp_store(name);
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 4400.0).framing();
    app.settings_open = true;
    d_frame_twice(&mut app, &mut d);
    (app, d, dir)
}

/// Settings → Sync's server typed, and Continue: asked what it is, the
/// answer waited for.
fn check_server(app: &mut Carnet, d: &mut Drive, typed: &str) {
    app.sync_server = typed.into();
    app.on_plain(Msg::SyncCheck);
    assert!(app.connect.checking() || app.connect.problem.is_some());
    until_done("checked", app, d, &|x| !x.connect.checking());
    d_frame_twice(app, d);
}

/// A server of one's own, taking only codes: nothing to choose between,
/// so straight to the code (the words still there), two steps in all;
/// Back is to the server, as typed. An address nothing answers at, and
/// one where something not Carnet does, are said so on the first step.
#[test]
fn a_server_that_takes_only_codes_goes_straight_to_the_code() {
    use carnet_server::server::{self, Config};
    let data = std::env::temp_dir().join(format!("carnet-app-codes-only-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    let (mut a, mut d, dir) = tall("codes-only");
    assert!(drawn(&mut d, &mut a, "Continue"));

    // Something that takes the connection and never answers: asked
    // until Cancel.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    a.sync_server = format!("http://{}", silent.local_addr().unwrap());
    a.on_plain(Msg::SyncCheck);
    assert!(a.connect.checking() && drawn(&mut d, &mut a, "Checking\u{2026}"));
    let cancel = d.key_named("Cancel").expect("Cancel");
    d.click_key(&mut a, cancel);
    d_frame_twice(&mut a, &mut d);
    assert!(!a.connect.checking() && a.connect.problem.is_none() && drawn(&mut d, &mut a, "Continue"));
    drop(silent);
    // Nothing answers there.
    check_server(&mut a, &mut d, "http://127.0.0.1:1");
    assert_eq!(a.connect.problem, Some(carnet_sync::info::Problem::Unreachable));
    assert_eq!(a.connect.step(), crate::connect::Step::Server);
    assert!(drawn(&mut d, &mut a, &carnet_sync::info::Problem::Unreachable.words()));
    // A name the DNS has no address for: said so, by its name.
    check_server(&mut a, &mut d, "https://carnet-test.invalid");
    assert_eq!(a.connect.problem, Some(carnet_sync::info::Problem::DomainNotFound("carnet-test.invalid".into())));
    assert!(drawn(&mut d, &mut a, "Domain not found: carnet-test.invalid. Check the address; a new name can take a few minutes to appear."));
    // Something answers, not Carnet (the server, under a path it hasn't).
    check_server(&mut a, &mut d, &format!("{origin}/elsewhere"));
    assert_eq!(a.connect.problem, Some(carnet_sync::info::Problem::NotCarnet));
    assert!(drawn(&mut d, &mut a, &carnet_sync::info::Problem::NotCarnet.words()));
    assert!(drawn(&mut d, &mut a, "Cloud sync setup"), "still on the first step");

    // Codes only: the code's form, straight away.
    check_server(&mut a, &mut d, &origin);
    assert!(a.connect.problem.is_none());
    assert_eq!(a.connect.step(), crate::connect::Step::Form);
    assert!(drawn(&mut d, &mut a, "Step 2 of 2") && drawn(&mut d, &mut a, "Enter the code"));
    assert!(d.key_of("sync-code").is_some() && drawn(&mut d, &mut a, "Connect"));
    assert!(!drawn(&mut d, &mut a, "Email and password") && !drawn(&mut d, &mut a, "Make an account"));
    assert_eq!(d.core.focus().and_then(|k| a.fields.get(&k)).copied(), Some(Field::SyncCode), "the code's field has the keyboard");
    // The words, from here; and back to a code.
    let words = d.key_named("Use recovery words").expect("the words, still");
    d.click_key(&mut a, words);
    d_frame_twice(&mut a, &mut d);
    assert_eq!(a.sync_how, 1);
    assert!(drawn(&mut d, &mut a, "Recover"));
    assert!(drawn(&mut d, &mut a, "Use a code instead"));
    a.on_plain(Msg::SyncHow { to: 0 });
    d_frame_twice(&mut a, &mut d);
    // A wrong code: said on this step.
    a.sync_code = "47-12".into();
    a.on_plain(Msg::SyncStart);
    d_frame_twice(&mut a, &mut d);
    assert!(a.sync.error.as_deref().is_some_and(|e| e.starts_with("That isn't a code")));
    assert!(drawn(&mut d, &mut a, "Connect"), "on the code's step still");
    // Back: to the server, as typed.
    let back = d.key_named("Back").expect("Back");
    d.click_key(&mut a, back);
    d_frame_twice(&mut a, &mut d);
    assert_eq!(a.connect.step(), crate::connect::Step::Server);
    assert!(a.sync.error.is_none());
    assert_eq!(a.sync_server, origin);
    let field = app_field(&a, Field::SyncServer).expect("the server's field");
    assert_eq!(d.core.edit_text(field).as_deref(), Some(origin.as_str()), "the address kept in its field");

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir);
    let _ = std::fs::remove_dir_all(&data);
}

/// A hosted service that takes passwords and signups: every way in
/// offered, each a row; Change goes back to the server, its address kept;
/// Back from a way's form is to the ways.
#[test]
fn a_hosted_server_offers_every_way_in() {
    use carnet_server::server::{self, Config};
    let data = std::env::temp_dir().join(format!("carnet-app-every-way-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let host = server::Host { signup: Some(format!("{origin}/start")), passwords: true, first_invite: false, ..server::Host::default() };
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, host, async {
        let _ = stopped.await;
    }));
    let (mut a, mut d, dir) = tall("every-way");
    check_server(&mut a, &mut d, &origin);
    assert_eq!(a.connect.step(), crate::connect::Step::Way, "{:?}", a.connect.problem);
    assert!(!a.connect.checking() && a.connect.problem.is_none());
    assert!(drawn(&mut d, &mut a, "Step 2 of 3") && drawn(&mut d, &mut a, &origin) && drawn(&mut d, &mut a, "Carnet server, up to date"));
    for (key, name) in [("sync-way-0", "Enter the code"), ("sync-way-2", "Email and password"), ("sync-way-3", "Make an account")] {
        let k = d.key_of(key).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(d.key_named(name), Some(k), "{name}: a row, a button by its name");
    }
    assert!(drawn(&mut d, &mut a, "A code from another of your devices, or an invite"));
    assert!(drawn(&mut d, &mut a, "The ones set on another of your devices"));
    assert!(drawn(&mut d, &mut a, "We'll email you an invite"));
    assert!(drawn(&mut d, &mut a, "Use recovery words"));
    assert!(d.key_of("sync-code").is_none(), "no form before a way is chosen");

    // Email and password: its form, and Back to the ways.
    let row = d.key_of("sync-way-2").unwrap();
    d.click_key(&mut a, row);
    d_frame_twice(&mut a, &mut d);
    assert!(drawn(&mut d, &mut a, "Step 3 of 3") && drawn(&mut d, &mut a, "Sign in"));
    assert!(app_field(&a, Field::SyncEmail).is_some() && app_field(&a, Field::SyncPassword).is_some());
    assert_eq!(d.core.focus().and_then(|k| a.fields.get(&k)).copied(), Some(Field::SyncEmail));
    let back = d.key_named("Back").expect("Back");
    d.click_key(&mut a, back);
    d_frame_twice(&mut a, &mut d);
    assert_eq!(a.connect.step(), crate::connect::Step::Way);
    assert!(drawn(&mut d, &mut a, "Make an account"));

    // Change: the server's step, its address as typed.
    let change = d.key_named("Change").expect("Change");
    d.click_key(&mut a, change);
    d_frame_twice(&mut a, &mut d);
    assert_eq!(a.connect.step(), crate::connect::Step::Server);
    let field = app_field(&a, Field::SyncServer).expect("the server's field");
    assert_eq!(d.core.edit_text(field).as_deref(), Some(origin.as_str()));
    assert_eq!(d.core.focus(), Some(field), "the keyboard in it");

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir);
    let _ = std::fs::remove_dir_all(&data);
}

/// A passkey made in a browser shows in the app's Settings → Sync by its
/// name, and goes when Remove is asked twice: Cancel, or Yes, remove.
#[test]
fn the_accounts_passkeys_are_listed_and_removed() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-passkey-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, server::Host::default(), async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("passkey-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("online", &mut a, &mut da, &|x| x.sync.status == crate::sync::Status::Online);
    a.settings_open = true;
    a.sync.add_passkey_for_test("Passkey from Safari on iPhone", b"credential", &[5; 32]);
    until("listed", &mut a, &mut da, &|x| x.sync.passkeys.len() == 1);
    assert!(drawn(&mut da, &mut a, "Passkey from Safari on iPhone"));
    assert!(drawn(&mut da, &mut a, "Passkeys"));

    a.on_plain(Msg::SyncPasskeyRemove { at: 0 });
    assert!(drawn(&mut da, &mut a, "Yes, remove") && drawn(&mut da, &mut a, "Cancel"), "asked first");
    a.on_plain(Msg::SyncPasskeyCancel);
    assert!(!drawn(&mut da, &mut a, "Yes, remove"));
    a.on_plain(Msg::SyncPasskeyRemove { at: 0 });
    a.on_plain(Msg::SyncPasskeyRemove { at: 0 });
    until("removed", &mut a, &mut da, &|x| x.sync.passkeys.is_empty());
    assert!(!drawn(&mut da, &mut a, "Passkey from Safari on iPhone"));

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir_a);
    let _ = std::fs::remove_dir_all(&data);
}

/// A server that says times no day can be (`u64::MAX` seconds since 1970)
/// and sizes past counting, for devices, passkeys, the plan, invites,
/// accounts and the inspector: Settings → Sync draws, says such a time is
/// unknown, and never stops the app.
#[test]
fn settings_sync_draws_a_servers_impossible_times() {
    use carnet_server::server::{self, Config};
    use carnet_sync::session::{DeviceInfo, PasskeyInfo, Usage};
    use carnet_sync::wire::{AccountRow, BaseRow, InviteRow, LogRow, RoomRow};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-far-times-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, server::Host::default(), async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let (mut a, mut d, dir) = tall("far-times");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("online", &mut a, &mut d, &|x| x.sync.status == crate::sync::Status::Online);
    a.settings_open = true;
    until("listed", &mut a, &mut d, &|x| x.sync.devices.is_some() && x.sync.usage_settled() && x.sync.invites.is_some());
    let far = u64::MAX;
    let mut devices = a.sync.devices.clone().unwrap();
    devices.push(DeviceInfo { id: vec![9; 16], name: Some("Old phone".into()), seen: far, you: false, online: false });
    a.sync.devices = Some(devices);
    a.sync.passkeys = vec![PasskeyInfo { id: [7; 32], name: Some("A far passkey".into()), credential: vec![1], made: far }];
    a.sync.usage = Some(Usage { notes: far, files: far, limit: Some(far), writable: true, plan: "Forever".into(), until: Some(far) });
    a.sync.invites = Some(crate::sync::Invites {
        owner: true,
        waiting: vec![InviteRow { hash: [3; 32], until: far }],
        accounts: vec![AccountRow { id: vec![4; 16], owner: false, made: far, seen: far, devices: far, you: false }],
    });
    let base = BaseRow { by: vec![9; 16], at: far, verified: true, undoable: true, vv: vec![(1, far)], bytes: far };
    let room = RoomRow { room: "ab12".into(), held: vec![(1, far), (2, far)], live: far, live_bytes: far, base: Some(base), shelved: far, shelved_bytes: far, shelved_since: far };
    a.sync.inspection = Some((vec![room], vec![LogRow { at: far, device: vec![9; 16], room: "ab12".into(), what: "set the password".into() }]));
    a.sync_inspector = true;
    let shows = |d: &Drive, part: &str| d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.contains(part)));
    let _ = drawn(&mut d, &mut a, "");
    assert!(shows(&d, "Last seen at an unknown time"), "the device");
    assert!(shows(&d, "Added at an unknown time"), "the passkey");
    assert!(shows(&d, "\u{b7} Forever") && !shows(&d, "Forever, until"), "the plan, with no end said");
    assert!(shows(&d, "last seen at an unknown time"), "the account");

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    remove_test_dir(dir);
    let _ = std::fs::remove_dir_all(&data);
}

/// The account suspended, then the suspension lifted, while the app is
/// open (the host tells the hub: `limit_changed`): Settings says read only
/// at once, then takes it back; a note saved meanwhile reaches the server
/// with no restart; the plan's line is the new one.
#[test]
fn the_app_learns_at_once_its_account_can_write_again() {
    use carnet_server::quota::{Limit, Quota};
    use carnet_server::server::{self, Config, HubHandle};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    struct Plan(Mutex<Limit>);
    impl Quota for Plan {
        fn limit(&self, _: &[u8]) -> Limit {
            self.0.lock().unwrap().clone()
        }
    }
    let data = std::env::temp_dir().join(format!("carnet-app-limit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let plan = Arc::new(Plan(Mutex::new(Limit { bytes: Some(1 << 20), writable: true, plan: "Personal".into(), until: 0 })));
    let handle: Arc<Mutex<Option<HubHandle>>> = Arc::default();
    let h = handle.clone();
    let host = server::Host { quota: plan.clone(), first_invite: false, ready: Some(Box::new(move |hub| *h.lock().unwrap() = Some(hub))), ..server::Host::default() };
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, host, async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(15), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let (mut a, mut da, dir_a) = fresh("limit-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("online", &mut a, &mut da, &|x| x.sync.status == crate::sync::Status::Online);
    a.settings_open = true;
    until("the plan shown", &mut a, &mut da, &|x| x.sync.usage.as_ref().is_some_and(|u| u.plan == "Personal"));
    let hub = handle.lock().unwrap().clone().unwrap();
    let account = rt.block_on(async move { hub.run(|hub, _| hub.store().account_list()[0].0.clone()).await.unwrap() });
    let tell = || {
        let hub = handle.lock().unwrap().clone().unwrap();
        let account = account.clone();
        rt.block_on(async move { hub.limit_changed(account).await.unwrap() });
    };
    let held = || {
        let hub = handle.lock().unwrap().clone().unwrap();
        let account = account.clone();
        rt.block_on(async move { hub.run(move |hub, _| hub.store().usage(&account)).await.unwrap() })
    };

    plan.0.lock().unwrap().writable = false;
    tell();
    until("read only, said", &mut a, &mut da, &|x| x.sync.read_only);
    // Said once: in the account's card, not again as an error below it.
    da.core.set_inspect(true);
    da.frame(&mut a);
    let said = da.core.nodes().iter().filter(|n| n.text.as_deref().is_some_and(|t| t.contains("Read only for now") || t.contains("read only for now"))).count();
    assert_eq!(said, 1, "read only, said once");
    assert_eq!(a.sync.error, None);
    let before = held();
    // Written on the page, Settings away a moment: typed with Settings
    // over the page, it went nowhere, and what the wait below saw sent was
    // whatever else the app had to send, in its own time.
    a.settings_open = false;
    a.on_plain(Msg::FocusTitle);
    d_frame_twice(&mut a, &mut da);
    type_in(&mut a, &mut da, "Typed while suspended");
    assert_eq!(a.store.notes[a.current].title, "Typed while suspended");
    a.save_current();
    a.settings_open = true;
    for _ in 0..50 {
        da.frame(&mut a);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(held(), before, "nothing taken");

    *plan.0.lock().unwrap() = Limit { bytes: Some(1 << 30), writable: true, plan: "Family".into(), until: 0 };
    tell();
    until("writable again, the plan new", &mut a, &mut da, &|x| !x.sync.read_only && x.sync.usage.as_ref().is_some_and(|u| u.plan == "Family"));
    da.frame(&mut a);
    assert!(!da.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.contains("Read only for now"))), "taken back");
    let t0 = Instant::now();
    while held() == before {
        assert!(t0.elapsed() < Duration::from_secs(15), "timed out: what waited sent");
        da.frame(&mut a);
        std::thread::sleep(Duration::from_millis(5));
    }

    a.settings_open = false;
    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    drop(a);
    remove_test_dir(dir_a);
    let _ = std::fs::remove_dir_all(&data);
}

/// A picture pasted while the account is over its plan is held back (the
/// server says it's full, and Settings says so); the plan raised while
/// the app is open (the host tells the hub), the picture goes at once,
/// with no restart and Settings not even open, and "full" goes with it.
#[test]
fn a_picture_held_back_over_the_plan_goes_once_the_plan_is_raised() {
    use carnet_server::quota::{Limit, Quota};
    use carnet_server::server::{self, Config, HubHandle};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    struct Plan(Mutex<Limit>);
    impl Quota for Plan {
        fn limit(&self, _: &[u8]) -> Limit {
            self.0.lock().unwrap().clone()
        }
    }
    let data = std::env::temp_dir().join(format!("carnet-app-held-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    // A plan the notes alone fill: no picture goes.
    let plan = Arc::new(Plan(Mutex::new(Limit { bytes: Some(1), writable: true, plan: "Tiny".into(), until: 0 })));
    let handle: Arc<Mutex<Option<HubHandle>>> = Arc::default();
    let h = handle.clone();
    let host = server::Host { quota: plan.clone(), first_invite: false, ready: Some(Box::new(move |hub| *h.lock().unwrap() = Some(hub))), ..server::Host::default() };
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, None, host, async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(15), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let named = |x: &Carnet| x.store.replica.as_ref().is_some_and(|r| r.vault().files().keys().any(|p| p.starts_with("attachments/")));
    let (mut a, mut da, dir_a) = fresh("held-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("online", &mut a, &mut da, &|x| x.sync.status == crate::sync::Status::Online);
    key(&mut a, &mut da, "enter");
    type_in(&mut a, &mut da, "Over the plan");
    let rgba: Vec<u8> = (0..12).flat_map(|_| [10u8, 20, 30, 255]).collect();
    a.paste_picture(4, 3, &rgba);
    a.save_current();
    until("held back, said", &mut a, &mut da, &|x| x.sync.error.as_deref() == Some(carnet_sync::session::FULL));
    for _ in 0..40 {
        da.frame(&mut a);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!named(&a), "not sent while full");
    assert_eq!(a.sync.files_waiting(), 1, "counted as waiting, not up to date");

    let hub = handle.lock().unwrap().clone().unwrap();
    let account = rt.block_on(async move { hub.run(|hub, _| hub.store().account_list()[0].0.clone()).await.unwrap() });
    *plan.0.lock().unwrap() = Limit { bytes: Some(1 << 30), writable: true, plan: "Family".into(), until: 0 };
    let hub = handle.lock().unwrap().clone().unwrap();
    rt.block_on(async move { hub.limit_changed(account).await.unwrap() });
    // Well before the slow retry: the room said is what sends it.
    let t0 = Instant::now();
    until("sent, once there's room", &mut a, &mut da, &named);
    assert!(t0.elapsed() < Duration::from_secs(10));
    assert_eq!(a.sync.error, None, "full, taken back");
    until("nothing waiting", &mut a, &mut da, &|x| x.sync.files_waiting() == 0);

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    drop(a);
    remove_test_dir(dir_a);
    let _ = std::fs::remove_dir_all(&data);
}

/// Settings' list of devices is fresh while it's open: a device that goes
/// offline is no longer "Online now" without Settings being opened again
/// (the server tells), and opening Settings asks the server again.
#[test]
fn the_device_list_says_who_is_online_as_they_come_and_go() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-online-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(apps[0].0) {
            assert!(t0.elapsed() < Duration::from_secs(15), "timed out: {what}");
            for (app, d) in apps.iter_mut() {
                d.frame(&mut **app);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let laptop_online = |x: &Carnet| x.sync.devices.as_ref().and_then(|l| l.iter().find(|d| !d.you).map(|d| d.online));
    let (mut a, mut da, dir_a) = fresh("online-a");
    let (mut b, mut db, dir_b) = fresh("online-b");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("a online", &mut [(&mut a, &mut da)], &|x| x.sync.status == crate::sync::Status::Online);
    a.settings_open = true;
    a.on_plain(Msg::SyncPair);
    until("a code", &mut [(&mut a, &mut da)], &|x| x.sync.code.is_some());
    b.sync_server = origin.clone();
    b.sync_code = a.sync.code.clone().unwrap();
    b.on_plain(Msg::SyncStart);
    {
        let mut both = [(&mut a, &mut da), (&mut b, &mut db)];
        until("b listed, online now", &mut both, &|x| laptop_online(x) == Some(true));
    }
    // b goes (quit): a's list, open all along, says so unasked.
    drop(b);
    until("b no longer online", &mut [(&mut a, &mut da)], &|x| laptop_online(x) == Some(false));
    // Settings closed and opened again: asked again (the list stays shown
    // meanwhile).
    a.settings_open = false;
    da.frame(&mut a);
    a.settings_open = true;
    da.frame(&mut a);
    assert!(a.sync.devices.is_some(), "the list stays while asked again");

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    drop(a);
    remove_test_dir(dir_a);
    remove_test_dir(dir_b);
    let _ = std::fs::remove_dir_all(&data);
}

/// A device paired into an account that already has more notes than the
/// replica keeps in memory gets every one of them, as files, and they
/// stay its rooms after a restart; its inspector finds nothing missing.
#[test]
fn a_device_paired_late_gets_every_note() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-late-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    #[allow(clippy::type_complexity)]
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&[(&mut Carnet, &mut Drive)]) -> bool| {
        let t0 = Instant::now();
        while !done(apps) {
            assert!(t0.elapsed() < Duration::from_secs(30), "timed out: {what}");
            for (app, d) in apps.iter_mut() {
                d.frame(&mut **app);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("late-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    const MANY: usize = 45;
    for k in 0..MANY {
        let mut n = Note::new(&format!("Note {k:02}"), Doc::new(vec![Block::text(format!("words of note {k}"))]));
        n.folder = String::new();
        a.store.notes.push(n);
        let i = a.store.notes.len() - 1;
        a.store.save(i, true).unwrap();
    }
    until("A has sent them all", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.due().is_none() && x[0].0.sync.is_idle() && x[0].0.store.notes.iter().filter(|n| n.key.is_some()).count() > MANY);
    a.on_plain(Msg::SyncPair);
    until("a code", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.code.is_some());

    let (store, dir_b) = temp_store("late-b");
    let mut b = Carnet::new(store);
    let mut db = Drive::new(Core::new(), 1100.0, 760.0).framing();
    db.frame(&mut b);
    b.sync_server = origin.clone();
    b.sync_code = a.sync.code.clone().unwrap();
    b.on_plain(Msg::SyncStart);
    let titles = |s: &Store| {
        let mut t: Vec<String> = s.notes.iter().filter(|n| n.path.is_some()).map(|n| n.title.clone()).collect();
        t.sort();
        t
    };
    until("B has every note", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| titles(&x[0].0.store) == titles(&x[1].0.store));
    assert_eq!(titles(&b.store).len(), MANY + 1);
    // B's own welcome note simply went: in nobody's Deleted.
    for _ in 0..100 {
        da.frame(&mut a);
        db.frame(&mut b);
        std::thread::sleep(Duration::from_millis(10));
    }
    let trashed = |dir: &std::path::Path| std::fs::read_dir(dir.join(".trash")).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".md")).collect::<Vec<_>>()).unwrap_or_default();
    eprintln!("A trash {:?} / B trash {:?}", trashed(&dir_a), trashed(&dir_b));
    assert!(trashed(&dir_a).is_empty(), "B's welcome note in A's Deleted");
    assert!(trashed(&dir_b).is_empty(), "B's welcome note in B's Deleted");
    b.refresh_trash();
    assert!(b.trash.is_empty(), "B's Deleted shows nothing");

    // Its inspector: nothing on the server it lacks.
    b.settings_open = true;
    b.on_plain(Msg::SyncInspector);
    until("inspected", &mut [(&mut b, &mut db)], &|x| x[0].0.sync.inspection.as_ref().is_some_and(|(rooms, _)| rooms.len() > MANY));
    let _ = drawn(&mut db, &mut b, "");
    let shows = |d: &Drive, s: &str| d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.contains(s)));
    assert!(!shows(&db, "not on this device") && !shows(&db, "on this device yet"), "B lacks a note the server has");
    // Notes counted, and only notes (not the vault).
    assert!(shows(&db, &format!("{} notes are up to date.", MANY + 1)));
    // One it lacked would be said so, first, in so many words.
    if let Some((rooms, _)) = b.sync.inspection.as_mut() {
        let mut lacking = rooms.iter().find(|r| r.room != carnet_sync::vault::ROOM).unwrap().clone();
        lacking.room = carnet_sync::Key::random().to_string();
        rooms.push(lacking);
    }
    let _ = drawn(&mut db, &mut b, "");
    assert!(shows(&db, "1 note on the server isn't on this device yet."));
    b.settings_open = false;
    // Every note is a room of B's, after a restart too.
    let rooms = |app: &Carnet| app.store.replica.as_ref().map(|r| carnet_sync::session::Docs::rooms(r).len()).unwrap_or(0);
    assert!(rooms(&b) > MANY + 1, "{} rooms", rooms(&b));

    a.settings_open = false;
    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    drop(a);
    drop(b);
    remove_test_dir(dir_a);
    remove_test_dir(dir_b);
    let _ = std::fs::remove_dir_all(&data);
}

/// Two whole apps and a real server: A sets up by an invite and writes a
/// note; B comes in by the pairing code A shows, and has it; B's edit
/// reaches A's open note.
#[test]
fn two_apps_sync_through_a_server() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-sync-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 4, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    #[allow(clippy::type_complexity)]
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&[(&mut Carnet, &mut Drive)]) -> bool| {
        let t0 = Instant::now();
        while !done(apps) {
            assert!(t0.elapsed() < Duration::from_secs(15), "timed out: {what}");
            for (app, d) in apps.iter_mut() {
                d.frame(&mut **app);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("sync-a");
    a.sync.set_name("Desk".into());
    a.sync_server = origin.clone();
    a.sync_how = 0;
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    type_in(&mut a, &mut da, "Ferry times");
    key(&mut a, &mut da, "enter");
    type_in(&mut a, &mut da, "the 9:40 from the north pier");
    a.save_current();
    a.on_plain(Msg::SyncPair);
    until("a code", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.code.is_some());
    let code = a.sync.code.clone().unwrap();
    // A's Settings show it as a QR code too: the web viewer on this
    // server, with the code in the link.
    a.settings_open = true;
    d_frame_twice(&mut a, &mut da);
    assert_eq!(a.pair_qr.as_ref().map(|(link, _)| link.clone()), Some(format!("{origin}/#pair={code}")));

    let (mut b, mut db, dir_b) = fresh("sync-b");
    b.sync.set_name("Laptop".into());
    b.sync_server = origin.clone();
    b.sync_code = code.clone();
    // B's Settings open: the code is in its field, as typed.
    let code_field = |app: &Carnet, d: &Drive| {
        let k = app.fields.iter().find(|(_, f)| matches!(f, Field::SyncCode)).map(|(k, _)| *k)?;
        d.core.edit_text(k)
    };
    b.settings_open = true;
    // The server asked first: one of one's own, straight to the code.
    b.on_plain(Msg::SyncCheck);
    until("B's server asked", &mut [(&mut b, &mut db)], &|x| x[0].0.connect.step() == crate::connect::Step::Form);
    d_frame_twice(&mut b, &mut db);
    assert_eq!(code_field(&b, &db).as_deref(), Some(code.as_str()));
    // The new note fresh() opened is B's own: it stays.
    let mine = b.store.notes[b.current].id;
    b.on_plain(Msg::SyncStart);
    // B may have the note before A has heard the pairing is done: wait
    // for both.
    until("B has the note, and A says a device is in", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        x[1].0.store.notes.iter().any(|n| n.title == "Ferry times")
            && x[0].0.notice.as_ref().is_some_and(|n| n.text.starts_with("A new device is in"))
    });
    assert!(b.sync.is_on());
    assert!(b.sync_code.is_empty(), "the code doesn't stay on screen once used");
    assert!(a.sync.joined.is_some(), "Settings says so where the code was");
    // Said in Settings while they're open; once they're left, seen: not
    // said again the next time (as the web lets it go).
    d_frame_twice(&mut a, &mut da);
    assert!(a.joined_said, "said in Settings");
    assert!(a.sync.joined.is_some(), "still said while Settings are open");
    a.settings_open = false;
    d_frame_twice(&mut a, &mut da);
    assert!(a.sync.joined.is_none() && !a.joined_said, "Settings left: let go");
    a.settings_open = true;
    d_frame_twice(&mut a, &mut da);
    assert!(!a.joined_said, "not said again");
    // B's own welcome note, untouched, went (not to its Deleted) and was
    // never sent. Each has the same notes on disk, and only those.
    let titles = |s: &Store| {
        let mut t: Vec<String> = s.notes.iter().filter(|n| n.path.is_some()).map(|n| n.title.clone()).collect();
        t.sort();
        t
    };
    until("B has exactly A's notes", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        titles(&x[0].0.store) == titles(&x[1].0.store)
    });
    assert_eq!(titles(&a.store), ["Ferry times", "Welcome to Carnet"], "one welcome note, not two");
    assert!(b.store.notes.iter().all(|n| !n.is_blank() || n.id == mine), "no empty stand-in besides");
    assert!(b.store.trashed().is_empty(), "nor the welcome note in B's Deleted");

    // B edits; A, with the note open and no edits of its own, shows it.
    let i = b.store.notes.iter().position(|n| n.title == "Ferry times").unwrap();
    b.open(i);
    d_frame_twice(&mut b, &mut db);
    let note = &mut b.store.notes[b.current];
    note.doc.push(Block::text("and the 17:10 back"));
    note.touch();
    b.save_current();
    until("A has B's edit", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        x[0].0.doc().blocks().iter().any(|b| b.text_ref() == Some("and the 17:10 back"))
    });
    // A's history says who: B's edit on the laptop, by its name; A's own
    // here. (The devices are listed while the history shows.)
    a.on_plain(Msg::ShowHistory);
    until("A knows the devices", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| x[0].0.sync.devices.as_ref().is_some_and(|l| l.len() == 2));
    assert!(drawn(&mut da, &mut a, "Edited on Laptop"), "B's edit, by B's name");
    assert!(drawn(&mut da, &mut a, "Edited here"), "A's own");
    a.on_plain(Msg::CloseHistory);

    // Where notes live follows too (the vault doc): a folder made and a
    // note pinned and moved into it on A are so on B; a note trashed on A
    // is in B's Deleted.
    a.store.create_folder("", "Travel").unwrap();
    let i = a.store.notes.iter().position(|n| n.title == "Ferry times").unwrap();
    a.store.notes[i].pinned = true;
    a.store.move_note(i, "Travel").unwrap();
    until("B has it pinned, in Travel", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        x[1].0.store.notes.iter().any(|n| n.title == "Ferry times" && n.folder == "Travel" && n.pinned)
    });
    let mut scratch = Note::new("Scratch", Doc::new(vec![Block::text("to go")]));
    scratch.folder = String::new();
    a.store.notes.push(scratch);
    let i = a.store.notes.len() - 1;
    a.store.save(i, true).unwrap();
    until("B has the scratch note", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        x[1].0.store.notes.iter().any(|n| n.title == "Scratch")
    });
    let i = a.store.notes.iter().position(|n| n.title == "Scratch").unwrap();
    a.store.trash(i).unwrap();
    until("B has it in Deleted", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        x[1].0.trash.iter().any(|t| t.title == "Scratch") && x[1].0.store.notes.iter().all(|n| n.title != "Scratch")
    });

    // A picture pasted on A is on B, the same file at the same path.
    let photo: Vec<u8> = (0..400_000u32).map(|i| (i % 253) as u8).collect();
    let src = a.store.save_attachment("", "png", &photo).unwrap();
    let there = dir_b.join(&src);
    until("B has the picture", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|_| std::fs::read(&there).ok().as_deref() == Some(photo.as_slice()));

    // C as the app is on its first start: the welcome note its only
    // note, and open. Joining takes it away; an empty note stands in
    // until A's arrive, then goes too.
    let (store, dir_c) = temp_store("sync-c");
    let mut c = Carnet::new(store);
    let mut dc = Drive::new(Core::new(), 1100.0, 760.0).framing();
    dc.frame(&mut c);
    c.sync.set_name("Tablet".into());
    a.on_plain(Msg::SyncPair);
    until("a second code", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.code.is_some());
    c.sync_server = origin.clone();
    c.sync_code = a.sync.code.clone().unwrap();
    c.on_plain(Msg::SyncStart);
    until("C has exactly A's notes", &mut [(&mut a, &mut da), (&mut c, &mut dc)], &|x| {
        titles(&x[0].0.store) == titles(&x[1].0.store) && x[1].0.store.notes.len() == x[0].0.store.notes.len()
    });
    assert!(c.store.notes.iter().all(|n| !n.is_blank()), "the stand-in went");
    assert_eq!(c.store.notes[c.current].title, c.store.notes[c.order[0]].title, "a note of the account's open");

    // Settings → Sync on A lists the three, by their names. A removes the
    // laptop (asked twice): the laptop finds itself out and forgets its
    // keys; the tablet takes the new key and goes on syncing.
    a.settings_open = true;
    // What the account keeps is asked with them: answered too, or the
    // answer would take the place of the plan put in below.
    until("A lists three devices", &mut [(&mut a, &mut da), (&mut b, &mut db), (&mut c, &mut dc)], &|x| {
        x[0].0.sync.devices.as_ref().is_some_and(|l| l.len() == 3 && l.iter().all(|d| d.name.is_some())) && x[0].0.sync.usage_settled()
    });
    let list = a.sync.devices.clone().unwrap();
    let mut names: Vec<(String, bool)> = list.iter().map(|d| (d.name.clone().unwrap(), d.you)).collect();
    names.sort();
    assert_eq!(names, [("Desk".into(), true), ("Laptop".into(), false), ("Tablet".into(), false)]);
    // Each with its id, short, after what it is: two of one name told apart.
    let short = |id: &[u8]| id.iter().take(3).map(|b| format!("{b:02x}")).collect::<String>();
    let me = list.iter().find(|d| d.you).unwrap();
    assert!(drawn(&mut da, &mut a, "Laptop") && drawn(&mut da, &mut a, &format!("This device \u{b7} {}", short(&me.id))));
    // The others are logged in now: said so, not when their session began.
    let laptop = list.iter().find(|d| d.name.as_deref() == Some("Laptop")).unwrap();
    assert!(laptop.online && list.iter().all(|d| d.online));
    assert!(drawn(&mut da, &mut a, &format!("Online now \u{b7} {}", short(&laptop.id))));
    // Over a (tiny) plan's limit: said plainly, and what it means.
    a.sync.usage = Some(carnet_sync::session::Usage { notes: 700, files: 41, limit: Some(42), writable: true, plan: "Tiny".into(), until: None });
    assert!(drawn(&mut da, &mut a, "741 bytes of 42 bytes used \u{b7} Tiny"));
    assert!(drawn(&mut da, &mut a, "Over this plan's 42 bytes: new files stay on this device until there's room. Notes still sync."));
    // An invite waiting, its code in the vault, so on the laptop too.
    a.on_plain(Msg::SyncInvite);
    let waiting = carnet_sync::invite::InviteCode::parse(&a.sync.invite.clone().expect("an invite")).unwrap();
    until("the laptop has the invite's code", &mut [(&mut a, &mut da), (&mut b, &mut db), (&mut c, &mut dc)], &|x| {
        x[1].0.store.invite_code(&waiting.hash()).is_some() && x[0].0.sync.invites.as_ref().is_some_and(|i| i.waiting.len() == 1)
    });
    let at = list.iter().position(|d| d.name.as_deref() == Some("Laptop")).unwrap() as u64;
    // Asked first: Cancel, or Yes, remove. Cancel keeps it.
    a.on_plain(Msg::SyncRemove { at });
    assert!(b.sync.is_on() && a.sync_remove_armed.is_some(), "asked once: not yet");
    assert!(drawn(&mut da, &mut a, "Cancel") && drawn(&mut da, &mut a, "Yes, remove"));
    a.on_plain(Msg::SyncRemoveCancel);
    assert!(a.sync_remove_armed.is_none() && !drawn(&mut da, &mut a, "Yes, remove"));
    a.on_plain(Msg::SyncRemove { at });
    a.on_plain(Msg::SyncRemove { at });
    // The laptop had every waiting invite's code: each taken back, and
    // said so.
    assert!(a.notice.as_ref().is_some_and(|n| n.text == "Waiting invites were taken back"));
    assert!(a.sync.invite.is_none() && a.store.invite_code(&waiting.hash()).is_none());
    until("the laptop is out", &mut [(&mut a, &mut da), (&mut b, &mut db), (&mut c, &mut dc)], &|x| {
        !x[1].0.sync.is_on() && x[0].0.sync.devices.as_ref().is_some_and(|l| l.len() == 2)
    });
    until("no invite waits, nor its code on the tablet", &mut [(&mut a, &mut da), (&mut c, &mut dc)], &|x| {
        x[0].0.sync.invites.as_ref().is_some_and(|i| i.waiting.is_empty()) && x[1].0.store.invite_code(&waiting.hash()).is_none()
    });
    assert!(b.sync.error.as_deref().is_some_and(|e| e.contains("removed")), "{:?}", b.sync.error);
    // Off again, its Settings start from the server, its address kept;
    // then an empty code field, not the old code.
    b.settings_open = true;
    d_frame_twice(&mut b, &mut db);
    assert!(drawn(&mut db, &mut b, "Cloud sync setup"));
    assert_eq!(b.sync_server, origin);
    b.on_plain(Msg::SyncCheck);
    until("B's server asked again", &mut [(&mut b, &mut db)], &|x| x[0].0.connect.step() == crate::connect::Step::Form);
    d_frame_twice(&mut b, &mut db);
    assert!(drawn(&mut db, &mut b, "Connect"));
    assert_eq!(code_field(&b, &db).as_deref(), Some(""), "the old code left in the field");
    let i = a.store.notes.iter().position(|n| n.title == "Ferry times").unwrap();
    a.open(i);
    d_frame_twice(&mut a, &mut da);
    let note = &mut a.store.notes[a.current];
    note.doc.push(Block::text("and the 21:00, the last"));
    note.touch();
    a.save_current();
    until("the tablet has it, on the new key", &mut [(&mut a, &mut da), (&mut c, &mut dc)], &|x| {
        x[1].0.store.notes.iter().any(|n| n.doc.blocks().iter().any(|b| b.text_ref() == Some("and the 21:00, the last")))
    });
    assert!(!b.store.notes.iter().any(|n| n.doc.blocks().iter().any(|b| b.text_ref() == Some("and the 21:00, the last"))));
    a.settings_open = false;
    c.on_plain(Msg::SyncOff);
    c.on_plain(Msg::SyncOff);
    drop(c);
    remove_test_dir(dir_c);

    // A alone now: saves past the server's limit (four records here) make
    // the note a snapshot. The inspector shows it, and rolls it back,
    // asked twice.
    for k in 0..5 {
        let i = a.store.notes.iter().position(|n| n.title == "Ferry times").unwrap();
        let note = &mut a.store.notes[i];
        note.doc.push(Block::text(format!("note {k}")));
        note.touch();
        a.save_current();
        until("saved", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    }
    a.settings_open = true;
    a.on_plain(Msg::SyncInspector);
    let ferry = a.store.notes.iter().find(|n| n.title == "Ferry times").and_then(|n| n.key).unwrap().to_string();
    // Asked again until the server has it (the snapshot goes after the
    // saves; the first answer may come before it).
    let t0 = Instant::now();
    while !a.sync.inspection.as_ref().is_some_and(|(rooms, _)| rooms.iter().any(|r| r.room == ferry && r.base.is_some())) {
        assert!(t0.elapsed() < Duration::from_secs(15), "timed out: a snapshot in the inspector");
        if a.sync.inspection.is_some() {
            a.on_plain(Msg::SyncInspectorRefresh);
        }
        da.frame(&mut a);
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = drawn(&mut da, &mut a, "");
    let shows = |d: &Drive, start: &str| d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.starts_with(start)));
    assert!(shows(&da, "Ferry times"), "the note by its title");
    assert!(shows(&da, "Up to date"));
    assert!(shows(&da, "Packed into one copy just now by this device"));
    let at = a.sync.inspection.as_ref().unwrap().0.iter().position(|r| r.room == ferry).unwrap() as u64;
    a.on_plain(Msg::SyncInspectMore { at });
    assert!(drawn(&mut da, &mut a, "Undo packing"), "its ways to fix it, shown");
    a.on_plain(Msg::SyncRollBack { at });
    assert!(drawn(&mut da, &mut a, "Cancel"), "asked first");
    a.on_plain(Msg::SyncInspectCancel);
    assert!(a.sync_inspect_armed.is_none());
    a.on_plain(Msg::SyncRollBack { at });
    a.on_plain(Msg::SyncInspectConfirm);
    until("rolled back", &mut [(&mut a, &mut da)], &|x| {
        x[0].0.sync.inspection.as_ref().is_some_and(|(rooms, log)| {
            rooms.iter().any(|r| r.room == ferry && r.base.is_none()) && log.first().is_some_and(|l| l.what == "rolled back")
        })
    });
    let _ = drawn(&mut da, &mut a, "");
    assert!(shows(&da, "Lately"));
    // Every line it draws has the card's width to wrap in, not a letter's.
    for start in ["Lately", "Ferry times: packing undone", "1 note is up to date", "notes are up to date"] {
        for n in da.core.nodes().iter().filter(|n| n.text.as_deref().is_some_and(|t| t.starts_with(start) || t.contains(start))) {
            assert!(n.rect.w > n.rect.h, "{:?} drawn {}×{}: squeezed", n.text, n.rect.w, n.rect.h);
        }
    }
    assert!(shows(&da, "Ferry times: packing undone"), "the log in plain words");
    a.on_plain(Msg::SyncInspector);
    a.settings_open = false;

    // Turn off asks first, and anything between disarms it: Cancel too.
    a.on_plain(Msg::SyncOff);
    assert!(a.sync.is_on() && a.sync_off_armed);
    a.settings_open = true;
    assert!(drawn(&mut da, &mut a, "Yes, turn off") && drawn(&mut da, &mut a, "Cancel"));
    a.on_plain(Msg::SyncOffCancel);
    assert!(a.sync.is_on() && !a.sync_off_armed);
    a.on_plain(Msg::SyncOff);
    a.on_plain(Msg::SyncHow { to: 0 });
    a.on_plain(Msg::SyncOff);
    assert!(a.sync.is_on(), "asked again after something else");
    a.on_plain(Msg::SyncOff);
    assert!(!a.sync.is_on());
    drop(a);
    drop(b);
    let _ = stop.send(());
    let _ = rt.block_on(served);
    remove_test_dir(dir_a);
    remove_test_dir(dir_b);
    let _ = std::fs::remove_dir_all(&data);
}

/// The server's owner invites someone from Settings: the code shown, the
/// invite waiting; they make an account with it, which isn't the owner's;
/// the owner sees it, and removes it (asked twice), and their app is told.
#[test]
fn the_owner_invites_someone_and_removes_their_account() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-invite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    #[allow(clippy::type_complexity)]
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&[(&mut Carnet, &mut Drive)]) -> bool| {
        let t0 = Instant::now();
        while !done(apps) {
            assert!(t0.elapsed() < Duration::from_secs(15), "timed out: {what}");
            for (app, d) in apps.iter_mut() {
                d.frame(&mut **app);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("invite-a");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    a.settings_open = true;
    until("A's the owner", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.invites.as_ref().is_some_and(|i| i.owner));
    assert!(drawn(&mut da, &mut a, "Invite someone") && drawn(&mut da, &mut a, "Make an invite"));
    assert!(!drawn(&mut da, &mut a, "Other accounts on this server"), "none yet");

    a.on_plain(Msg::SyncInvite);
    until("the invite waiting", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.invites.as_ref().is_some_and(|i| i.waiting.len() == 1));
    let code = a.sync.invite.clone().expect("the code shown");
    assert!(drawn(&mut da, &mut a, &code), "the code, big");
    assert!(drawn(&mut da, &mut a, "Not used yet \u{b7} good for 7 more days"));
    // Waiting, its code is words to select, and a Copy beside it.
    let nodes = da.core.nodes();
    let selectable = |key: Key| {
        let mut at = nodes.iter().find(|n| n.key == key);
        while let Some(n) = at {
            if n.flags.contains(&"selectable") {
                return true;
            }
            at = n.parent.and_then(|p| nodes.iter().find(|x| x.key == p));
        }
        false
    };
    let shown: Vec<Key> = nodes.iter().filter(|n| n.text.as_deref() == Some(code.as_str())).map(|n| n.key).collect();
    assert!(shown.len() == 2 && shown.iter().all(|k| selectable(*k)), "big, and in the list: {shown:?}");
    let copy = da.key_of("sync-invite-copy-0").expect("Copy");
    assert!(da.rect_of(copy).is_some());
    a.on_plain(Msg::SyncInviteCopy { at: 0 });
    da.frame(&mut a);
    assert_eq!(copied(&mut da).as_deref(), Some(code.as_str()));
    // Its code kept in the vault: shown by A's other devices too, and by
    // A after a restart.
    let hash = carnet_sync::invite::InviteCode::parse(&code).unwrap().hash();
    assert_eq!(a.store.invite_code(&hash).as_deref(), Some(code.as_str()));
    assert_eq!(crate::store::Store::open(dir_a.clone()).unwrap().invite_code(&hash).as_deref(), Some(code.as_str()));

    // Someone else makes their own account with it.
    let (mut b, mut db, dir_b) = fresh("invite-b");
    b.sync_server = origin.clone();
    b.sync_code = code.clone();
    b.on_plain(Msg::SyncStart);
    until("B online", &mut [(&mut b, &mut db)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    assert_ne!(b.sync.account(), a.sync.account(), "an account of its own, not a device of A's");
    b.settings_open = true;
    until("B is told it isn't the owner's", &mut [(&mut b, &mut db)], &|x| x[0].0.sync.invites.is_some());
    assert!(!b.sync.invites.as_ref().unwrap().owner);
    assert!(!drawn(&mut db, &mut b, "Invite someone"));

    // A's Settings, opened again: the invite used, and the new account;
    // the one the server printed at its start went when A came in.
    a.on_plain(Msg::SyncInviteDone);
    a.settings_open = false;
    da.frame(&mut a);
    a.settings_open = true;
    until("A lists B's account", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        x[0].0.sync.invites.as_ref().is_some_and(|i| i.waiting.is_empty() && i.accounts.len() == 2)
    });
    assert!(drawn(&mut da, &mut a, "Other accounts on this server"));
    let at = a.sync.invites.as_ref().unwrap().accounts.iter().position(|x| !x.you).unwrap() as u64;
    a.on_plain(Msg::SyncAccountRemove { at });
    assert!(a.sync_account_armed.is_some() && drawn(&mut da, &mut a, "Yes, remove"), "asked once: not yet");
    a.on_plain(Msg::SyncAccountRemoveCancel);
    assert!(a.sync_account_armed.is_none(), "Cancel keeps it");
    a.on_plain(Msg::SyncAccountRemove { at });
    a.on_plain(Msg::SyncAccountRemove { at });
    until("B's account is gone", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        !x[1].0.sync.is_on() && x[0].0.sync.invites.as_ref().is_some_and(|i| i.accounts.len() == 1)
    });
    assert!(b.sync.error.as_deref().is_some_and(|e| e.contains("isn't on the server any more")), "{:?}", b.sync.error);

    // A renamed: typed in a field on its own row, saved, and listed so.
    // (Kept in sync.txt: `sync::tests::config_and_keys_read_back`.)
    until("A's devices", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.devices.is_some());
    a.on_plain(Msg::SyncRename);
    assert!(a.sync_rename.is_some() && drawn(&mut da, &mut a, "Save"));
    let focused = da.core.focus().and_then(|k| a.fields.get(&k).cloned());
    assert!(matches!(focused, Some(Field::DeviceName)), "the field has the keyboard: {focused:?}");
    a.sync_rename = Some("  Studio   desk ".into());
    a.on_plain(Msg::SyncRenameSave);
    until("A listed by its new name", &mut [(&mut a, &mut da)], &|x| {
        x[0].0.sync.devices.as_ref().is_some_and(|l| l.iter().any(|d| d.you && d.name.as_deref() == Some("Studio desk")))
    });
    assert!(drawn(&mut da, &mut a, "Studio desk") && drawn(&mut da, &mut a, "Rename"));
    drop(a);
    drop(b);
    let _ = stop.send(());
    let _ = rt.block_on(served);
    remove_test_dir(dir_a);
    remove_test_dir(dir_b);
    let _ = std::fs::remove_dir_all(&data);
}

/// Keys two nodes of the last frame share, in the tree and in the access
/// tree: one the access tree has twice is a child given twice to
/// accesskit, which panics at it.
fn shared_keys(d: &mut Drive) -> Vec<String> {
    let mut twice = Vec::new();
    let mut seen = std::collections::HashMap::new();
    for n in d.core.nodes() {
        if let Some((text, label)) = seen.insert(n.key, (n.text.clone(), n.label.clone())) {
            twice.push(format!("{:?}: {text:?} / {label:?}, and {:?} / {:?}", n.key, n.text, n.label));
        }
    }
    let mut seen = std::collections::HashMap::new();
    for n in &d.core.access_tree().nodes {
        if let Some(first) = seen.insert(n.key, n.name.clone()) {
            twice.push(format!("in the access tree, {:?}: {first:?} and {:?}", n.key, n.name));
        }
    }
    twice
}

/// Removed from the account on another device (the web app, here a second
/// app), this desktop is let go; the owner makes an invite there, and it's
/// typed here into Settings → Sync and connected. No frame on the way has
/// two nodes under one key (a release build panicked in accesskit at it).
#[test]
fn removed_then_back_by_an_invite_draws_each_node_once() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-rejoin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    // Every frame of each, looked at: a duplicate seen the moment it's drawn.
    #[allow(clippy::type_complexity)]
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&[(&mut Carnet, &mut Drive)]) -> bool| {
        let t0 = Instant::now();
        loop {
            for (app, d) in apps.iter_mut() {
                d.core.set_inspect(true);
                d.frame(&mut **app);
                let twice = shared_keys(d);
                assert!(twice.is_empty(), "{what}: {twice:#?}");
                let w = d.warnings();
                assert!(w.is_empty(), "{what}: {w:?}");
            }
            if done(apps) {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(15), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    // The web app: the server's owner, Settings open the whole time.
    let (mut w, mut dw, dir_w) = fresh("rejoin-web");
    w.sync.set_name("Browser".into());
    w.sync_server = origin.clone();
    w.sync_code = invite.text();
    w.on_plain(Msg::SyncStart);
    until("the web app online", &mut [(&mut w, &mut dw)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    w.settings_open = true;
    w.on_plain(Msg::SyncPair);
    until("a pairing code", &mut [(&mut w, &mut dw)], &|x| x[0].0.sync.code.is_some() && x[0].0.sync.invites.as_ref().is_some_and(|i| i.owner));

    // The desktop comes in by it, and has Settings → Sync open.
    let (mut a, mut da, dir_a) = fresh("rejoin-desk");
    a.sync.set_name("Desk".into());
    a.sync_server = origin.clone();
    a.sync_code = w.sync.code.clone().unwrap();
    a.settings_open = true;
    a.on_plain(Msg::SyncStart);
    // The account has a passkey (the web app's way in), and the desktop
    // shows the notes on the server too.
    w.sync.add_passkey_for_test("Passkey from Safari on Mac", b"credential", &[5; 32]);
    a.on_plain(Msg::SyncInspector);
    until("both list two devices, the desktop the owner's, the passkey and the notes", &mut [(&mut w, &mut dw), (&mut a, &mut da)], &|x| {
        x.iter().all(|(app, _)| app.sync.devices.as_ref().is_some_and(|l| l.len() == 2))
            && x[1].0.sync.invites.as_ref().is_some_and(|i| i.owner)
            && x[1].0.sync.passkeys.len() == 1
            && x[1].0.sync.inspection.is_some()
    });

    // Removed from the web app: the desktop is told, and leaves sync.
    let at = w.sync.devices.as_ref().unwrap().iter().position(|d| !d.you).unwrap() as u64;
    w.on_plain(Msg::SyncRemove { at });
    w.on_plain(Msg::SyncRemove { at });
    until("the desktop is out", &mut [(&mut w, &mut dw), (&mut a, &mut da)], &|x| !x[1].0.sync.is_on());
    assert!(a.sync.error.as_deref().is_some_and(|e| e.contains("removed")), "{:?}", a.sync.error);

    // The owner makes an invite; it's typed into the desktop and connected.
    w.on_plain(Msg::SyncInvite);
    until("the invite", &mut [(&mut w, &mut dw), (&mut a, &mut da)], &|x| x[0].0.sync.invite.is_some());
    a.sync_server = origin.clone();
    a.sync_code = w.sync.invite.clone().unwrap();
    a.on_plain(Msg::SyncStart);
    until("the desktop online on an account of its own, its devices and invites listed", &mut [(&mut w, &mut dw), (&mut a, &mut da)], &|x| {
        let a = &x[1].0;
        a.sync.status == crate::sync::Status::Online
            && a.sync.devices.as_ref().is_some_and(|l| l.len() == 1)
            && a.sync.invites.as_ref().is_some_and(|i| !i.owner)
    });
    assert_ne!(a.sync.account(), w.sync.account());
    // The notes on the server, still shown: asked again until the
    // desktop's own have reached its new account.
    for _ in 0..200 {
        if a.sync.inspection.as_ref().is_some_and(|(rooms, _)| !rooms.is_empty()) {
            break;
        }
        if a.sync.inspection.is_some() {
            a.on_plain(Msg::SyncInspectorRefresh);
        }
        until("asked", &mut [(&mut w, &mut dw), (&mut a, &mut da)], &|_| true);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(a.sync.inspection.as_ref().is_some_and(|(rooms, _)| !rooms.is_empty()), "the notes on the server listed");
    // A while longer, as the app sits there.
    for _ in 0..40 {
        until("settled", &mut [(&mut w, &mut dw), (&mut a, &mut da)], &|_| true);
    }

    drop(a);
    drop(w);
    let _ = stop.send(());
    let _ = rt.block_on(served);
    remove_test_dir(dir_a);
    remove_test_dir(dir_w);
    let _ = std::fs::remove_dir_all(&data);
}

fn d_frame_twice(app: &mut Carnet, d: &mut Drive) {
    d.frame(app);
    d.frame(app);
}

/// Sizes and days as Settings → Sync says them.
#[test]
fn sizes_and_days_read_as_people_say_them() {
    use std::time::{Duration, UNIX_EPOCH};
    assert_eq!(super::settings_view::size_words(512), "512 bytes");
    assert_eq!(super::settings_view::size_words(340_000), "340 KB");
    assert_eq!(super::settings_view::size_words(1_234_000_000), "1.2 GB");
    assert_eq!(super::settings_view::size_words(5 << 30), "5.4 GB");
    assert_eq!(super::chrome::date_words(UNIX_EPOCH + Duration::from_secs(1_799_712_000)), "12 January 2027");
    assert_eq!(super::chrome::date_words(UNIX_EPOCH + Duration::from_secs(1_835_442_000)), "29 February 2028");
    assert_eq!(super::chrome::date_words(UNIX_EPOCH), "1 January 1970");
}

/// A note's date: within a day, how long ago; older, the day, its year
/// only when it is not this one's — on the page and, shorter, on a card.
#[test]
fn a_date_past_a_day_is_the_day() {
    use super::chrome::{short_when, when_words};
    use std::time::{Duration, UNIX_EPOCH};
    // Noon on 12 January 2027.
    let now = UNIX_EPOCH + Duration::from_secs(1_799_712_000 + 12 * 3600);
    let before = |s: u64| now - Duration::from_secs(s);
    assert_eq!(when_words(now, before(3 * 3600)), "3 hours ago");
    assert_eq!(short_when(now, before(3 * 3600)), "3h");
    assert_eq!(when_words(now, before(30)), "just now");
    assert_eq!(when_words(now, before(3 * 86_400)), "9 January");
    assert_eq!(short_when(now, before(3 * 86_400)), "9 Jan");
    assert_eq!(when_words(now, before(35 * 86_400)), "8 December 2026");
    assert_eq!(short_when(now, before(35 * 86_400)), "8 Dec 2026");
}

/// Notes exported from Apple Notes put in the notes folder: their own
/// `Attachments` is no folder in the list, its pictures move to the
/// attachments folder with the links, it says so, and they show.
#[test]
fn an_apple_notes_export_comes_in_with_its_pictures() {
    let (store, dir) = temp_store("export");
    drop(store);
    let at = dir.join("Archive").join("Attachments");
    png_file(&at, "IMG 0412.png");
    std::fs::write(at.join("Boarding pass.pdf"), "pdf").unwrap();
    std::fs::write(
        dir.join("Archive").join("Lisbon.md"),
        "# Lisbon\n\n![](Attachments/IMG%200412.png)\n\n[Boarding pass.pdf](Attachments/Boarding%20pass.pdf)\n",
    )
    .unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    assert_eq!(app.store.folders, ["Archive"]);
    let said = app.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    assert!(said == "Moved 1 picture and 1 other file to the attachments folder", "{said}");
    assert!(!at.exists(), "emptied, gone");
    assert!(dir.join("attachments").join("IMG-0412.png").exists());
    let i = app.store.notes.iter().position(|n| n.title == "Lisbon").unwrap();
    app.open(i);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    assert!(d.core.nodes().iter().any(|n| n.role == Some(Role::Image) && n.rect.w > 1.0), "the picture shows");
    assert!(!d.core.nodes().iter().any(|n| n.text.as_deref() == Some("Picture not found")));
    let _ = std::fs::remove_dir_all(dir);
}

/// A note from an Obsidian vault: its embeds show the pictures they find
/// by name, wherever in the notes folder they are, at the width they give;
/// an edit elsewhere writes them back as embeds. A note that embeds a note
/// shows the words as they are.
#[test]
fn an_obsidian_embed_shows_its_picture_and_stays_an_embed() {
    let (store, dir) = temp_store("wiki-picture");
    drop(store);
    png_file(&dir.join("Pictures").join("Trips"), "photo.png");
    let md = "# Trip\n\n![[photo.png|200]]\n\n![[Other note]]\n\nWords\n";
    std::fs::write(dir.join("Trip.md"), md).unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    let i = app.store.notes.iter().position(|n| n.title == "Trip").unwrap();
    app.open(i);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let pics: Vec<Rect> = d.core.nodes().iter().filter(|n| n.role == Some(Role::Image)).map(|n| n.rect).collect();
    assert!(pics.iter().any(|r| (r.w - 200.0).abs() < 0.5 && (r.h - 150.0).abs() < 0.5), "the picture shows, 200 wide: {pics:?}");
    assert!(!d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.starts_with("Picture not found"))));
    assert!(texts(&app).iter().any(|t| t == "![[Other note]]"), "{:?}", texts(&app));
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    // Words typed at the end; the embed is written as it was.
    let words = d.core.nodes().iter().find(|n| n.text.as_deref() == Some("Words")).unwrap().rect;
    d.click(&mut app, words.x + words.w / 2.0, words.y + words.h / 2.0);
    key(&mut app, &mut d, "end");
    type_in(&mut app, &mut d, " more");
    app.save_current();
    let written = std::fs::read_to_string(dir.join("Trip.md")).unwrap();
    assert_eq!(written, "# Trip\n\n![[photo.png|200]]\n\n![[Other note]]\n\nWords more\n");
    let _ = std::fs::remove_dir_all(dir);
}

/// An iPhone's HEIC photo: on macOS it shows, and one dropped in goes in
/// as a JPEG, which shows everywhere; elsewhere the card says where it
/// does show.
#[test]
fn a_heic_photo_shows_and_goes_in_as_a_jpeg() {
    let heic = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fixtures/halves.heic");
    let (store, dir) = temp_store("heic");
    drop(store);
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    std::fs::copy(&heic, dir.join("attachments").join("IMG_0001.HEIC")).unwrap();
    std::fs::write(dir.join("Photo.md"), "# Photo\n\n![](attachments/IMG_0001.HEIC)\n").unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    let i = app.store.notes.iter().position(|n| n.title == "Photo").unwrap();
    app.open(i);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let shown = |d: &Drive, s: &str| d.core.nodes().iter().any(|n| n.text.as_deref() == Some(s));
    if cfg!(target_os = "macos") {
        let pics: Vec<Rect> = d.core.nodes().iter().filter(|n| n.role == Some(Role::Image)).map(|n| n.rect).collect();
        assert!(pics.iter().any(|r| (r.w - 48.0).abs() < 0.5 && (r.h - 32.0).abs() < 0.5), "it shows, its own size: {pics:?}");
    } else {
        assert!(shown(&d, "HEIC pictures show on macOS; this one can't show here"));
    }
    #[cfg(target_os = "macos")]
    {
        let last = blocks(&app).last().unwrap().id;
        app.caret = Caret::at(Pos { slot: Slot::block(last), byte: 0 });
        app.insert_files(std::slice::from_ref(&heic));
        d.frame(&mut app);
        let srcs = image_blocks(&app);
        assert_eq!(srcs.last().map(String::as_str), Some("attachments/halves.jpg"), "{srcs:?}");
        let jpeg = std::fs::read(dir.join("attachments").join("halves.jpg")).unwrap();
        assert!(jpeg.starts_with(&[0xff, 0xd8]), "a JPEG");
        assert!(app.error.is_none(), "{:?}", app.error);
    }
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A folder with no notes but a file in it (a picture, the Finder's own)
/// is deleted to the trash, whole, as one with notes is: never a raw
/// "directory not empty".
#[test]
fn deleting_a_folder_with_only_other_files_moves_it_to_the_trash() {
    let (mut app, mut d, dir) = fresh("folder-files");
    let f = app.store.create_folder("", "Old").unwrap();
    std::fs::create_dir_all(dir.join("Old").join("Attachments")).unwrap();
    std::fs::write(dir.join("Old").join("Attachments").join("x.png"), "pic").unwrap();
    d.frame(&mut app);
    assert!(!app.folder_is_bare(&f), "the menu says Move folder to Deleted");
    app.folder_action(&f, "delete");
    assert!(app.error.is_none(), "{:?}", app.error);
    assert!(!dir.join("Old").exists());
    assert!(app.notice.as_ref().is_some_and(|n| n.text.starts_with("Moved \u{201c}Old\u{201d} to Deleted") && n.undo.is_some()));
    assert!(app.trash.iter().any(|t| t.is_folder && t.name == "Old"));
    // Nothing in it at all: just removed.
    let g = app.store.create_folder("", "Bare").unwrap();
    assert!(app.folder_is_bare(&g), "the menu says Delete folder");
    app.folder_action(&g, "delete");
    assert!(app.error.is_none() && !dir.join("Bare").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// Settings → Sync's lines of the account: at or over a plan's limit, what
/// that means (the server takes no new file, and every note's text);
/// changes and files waiting, counted.
#[test]
fn over_a_plans_limit_and_changes_waiting_are_said_plainly() {
    use super::settings_view::{over_limit, waiting_words};
    assert_eq!(over_limit(41, 42), None);
    assert_eq!(over_limit(741, 42).as_deref(), Some("Over this plan's 42 bytes: new files stay on this device until there's room. Notes still sync."));
    assert_eq!(over_limit(2_000_000, 2_000_000).as_deref(), Some("Reached this plan's 2.0 MB: new files stay on this device until there's room. Notes still sync."));
    assert_eq!(waiting_words(1, 0), "Waiting to send 1 change");
    assert_eq!(waiting_words(3, 0), "Waiting to send 3 changes");
    assert_eq!(waiting_words(0, 1), "Waiting to send 1 file");
    assert_eq!(waiting_words(2, 1), "Waiting to send 2 changes and 1 file");
}

/// A notice is one line. One too long for the room beside the note's meta
/// line is cut after a whole word, with "…", never mid-word; its whole
/// words are its name and tooltip. One that fits is drawn whole.
#[test]
fn a_notice_too_long_for_its_line_is_cut_at_a_word() {
    let (mut app, _, dir) = fresh("notice-cut");
    let mut d = Drive::new(Core::new(), 820.0, 700.0).framing();
    d.core.set_inspect(true);
    let long = "Changed in another app too: yours is kept as \u{201c}Ferry times, the summer timetable with every crossing\u{201d}";
    app.notify(long, Some(Undo::Edit));
    d_frame_twice(&mut app, &mut d);
    let key = d.core.access_tree().nodes.iter().find(|n| n.name.as_deref() == Some(long)).map(|n| n.key).expect("named by its whole words");
    let notice = d.core.nodes().into_iter().find(|n| n.key == key).unwrap();
    let tip = d.core.access_tree().nodes.iter().find(|n| n.key == key).and_then(|n| n.description.clone());
    assert_eq!(tip.as_deref(), Some(long), "its tooltip");
    // What it shows: its text, whole in the access tree.
    let shown = d.core.access_tree().nodes.iter().find(|n| n.parent == Some(key)).and_then(|n| n.name.clone()).expect("its words, cut");
    assert!(shown.ends_with('\u{2026}') && shown.len() < long.len(), "{shown}");
    let kept = shown.trim_end_matches('\u{2026}');
    assert!(long.starts_with(kept) && long[kept.len()..].starts_with(' '), "cut after a whole word: {shown}");
    // It and its Undo fit the line.
    let line = d.core.nodes().into_iter().find(|n| Some(n.key) == notice.parent).unwrap();
    let undo = d.key_of("undo").and_then(|k| d.rect_of(k)).unwrap();
    assert!(undo.x + undo.w <= line.rect.x + line.rect.w + 0.5, "Undo shows: {undo:?} in {:?}", line.rect);
    // Short: whole, nothing more to name it.
    app.notify("Moved \u{201c}hill.png\u{201d} into Carnet", None);
    d_frame_twice(&mut app, &mut d);
    assert!(drawn(&mut d, &mut app, "Moved \u{201c}hill.png\u{201d} into Carnet"));
    // The rule itself: whole words, the stop before the cut left out.
    let w = |s: &str| s.chars().count() as f32;
    assert_eq!(super::page_view::fit_words("Copied it into Carnet: this note shows it", 99.0, w), None);
    assert_eq!(super::page_view::fit_words("Copied it into Carnet: this note shows it", 24.0, w).as_deref(), Some("Copied it into Carnet\u{2026}"));
    assert_eq!(super::page_view::fit_words("Copied it into Carnet: this note shows it", 3.0, w).as_deref(), Some("Copied\u{2026}"), "the first word stays");
    assert_eq!(super::page_view::fit_words("Supercalifragilistic", 3.0, w), None, "one word: nothing to cut at");
    let _ = std::fs::remove_dir_all(dir);
}

/// A picture whose file is gone (an export's folder deleted by hand) is a
/// small card saying so — on its own line or at an edge, never a sliver
/// as tall as the page — and a press on it selects it, to be deleted like
/// any block.
#[test]
fn a_missing_picture_is_a_small_card_that_selects_and_deletes() {
    for (n, float) in ["", "|left", "|right"].into_iter().enumerate() {
        let (store, dir) = temp_store(&format!("missing-pic-{n}"));
        drop(store);
        let md = format!("# Trip\n\nBefore the picture\n\n![IMG 0001{float}](Attachments/IMG%200001.jpeg)\n\nAfter words\n\nMore words\n");
        std::fs::write(dir.join("Trip.md"), md).unwrap();
        let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
        let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
        d.core.set_inspect(true);
        d.frame(&mut app);
        let i = app.store.notes.iter().position(|n| n.title == "Trip").unwrap();
        app.open(i);
        for _ in 0..6 {
            d.frame(&mut app);
        }
        let w = d.warnings();
        assert!(w.is_empty(), "{float}: {w:?}");
        let shown = |d: &Drive, s: &str| d.core.nodes().iter().any(|n| n.text.as_deref() == Some(s));
        assert!(shown(&d, "Picture not found"), "{float}: said plainly");
        assert!(shown(&d, "IMG 0001.jpeg"), "{float}: its file's name");
        let pic = blocks(&app).iter().find(|b| matches!(b.kind, Kind::Image { .. })).unwrap().id;
        let r = app.row_rects.iter().find(|(b, _)| *b == pic).map(|(_, r)| *r).unwrap();
        assert!(r.h < 100.0 && (float.is_empty() || r.w < 400.0), "{float}: a small card, {r:?}");
        let card = d.core.nodes().iter().find(|n| n.text.as_deref() == Some("Picture not found")).unwrap().rect;
        d.click(&mut app, card.x + card.w / 2.0, card.y + card.h / 2.0);
        d.frame(&mut app);
        let idx = app.doc().index_of(pic).unwrap();
        assert_eq!(app.caret, page::select_blocks(app.doc(), idx, idx), "{float}: a press selects it");
        key(&mut app, &mut d, "backspace");
        assert!(!blocks(&app).iter().any(|b| matches!(b.kind, Kind::Image { .. })), "{float}: deleted");
        assert_eq!(texts(&app), ["Before the picture", "After words", "More words"], "{float}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A picture whose file says it is bigger than Carnet reads is a card
/// saying so, as the app goes on; the note's other words are there.
#[test]
fn a_picture_too_big_to_read_says_so() {
    let (store, dir) = temp_store("too-big");
    drop(store);
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    std::fs::write(dir.join("attachments/huge.png"), crate::images::tests::png_saying(60_000, 60_000)).unwrap();
    std::fs::write(dir.join("Huge.md"), "# Huge\n\nBefore\n\n![](attachments/huge.png)\n\nAfter\n").unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.frame(&mut app);
    let i = app.store.notes.iter().position(|n| n.title == "Huge").unwrap();
    app.open(i);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    d.core.set_inspect(true);
    d.frame(&mut app);
    let shown = |s: &str| d.core.nodes().iter().any(|n| n.text.as_deref() == Some(s));
    assert!(shown("This picture is too big to show"));
    assert!(shown("After"));
    remove_test_dir(dir);
}

/// With sync on, a note from another device showing that device's own
/// picture (beside the note there, which by design doesn't sync) says
/// where the picture is, as the web does, in the same small card; one of
/// Carnet's own that isn't here is still "Picture not found".
#[test]
fn another_devices_own_picture_says_it_is_on_that_device() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-elsewhere-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    let (store, dir) = temp_store("elsewhere");
    drop(store);
    std::fs::create_dir_all(dir.join("Home")).unwrap();
    std::fs::write(dir.join("Home/Weekend.md"), "# Weekend\n\nThe hill\n\n![](hill.jpg)\n\n![](../attachments/gone.png)\n\nAfter\n").unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.frame(&mut app);
    let shown = |d: &Drive, s: &str| d.core.nodes().iter().any(|n| n.text.as_deref() == Some(s));
    let says_elsewhere = |d: &Drive| d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t == super::page_view::ELSEWHERE));
    let i = app.store.notes.iter().position(|n| n.title == "Weekend").unwrap();
    app.open(i);
    for _ in 0..6 {
        d.frame(&mut app);
    }
    d.core.set_inspect(true);
    d.frame(&mut app);
    assert!(!says_elsewhere(&d), "sync off: this is the only device");
    assert!(shown(&d, "Picture not found"));

    app.sync_server = origin.clone();
    app.sync_code = invite.text();
    app.on_plain(Msg::SyncStart);
    let t0 = Instant::now();
    while app.sync.status != crate::sync::Status::Online {
        assert!(t0.elapsed() < Duration::from_secs(15), "timed out: online");
        d.frame(&mut app);
        std::thread::sleep(Duration::from_millis(5));
    }
    let i = app.store.notes.iter().position(|n| n.title == "Weekend").unwrap();
    app.open(i);
    for _ in 0..6 {
        d.frame(&mut app);
    }
    assert!(says_elsewhere(&d), "the user's, on the device that has it");
    assert!(shown(&d, "hill.jpg"), "named");
    assert!(shown(&d, "Picture not found"), "Carnet's own, missing, as before");
    // In the box the picture takes (as the web has it): the text's width,
    // a picture's height at least. "Picture not found" stays a small card.
    let words = d.core.nodes().iter().find(|n| n.text.as_deref().is_some_and(|t| t == super::page_view::ELSEWHERE)).unwrap().rect;
    let card = d
        .core
        .nodes()
        .iter()
        .map(|n| n.rect)
        .filter(|r| r.x <= words.x && r.y <= words.y && r.x + r.w >= words.x + words.w && r.y + r.h >= words.y + words.h && r.h >= 100.0)
        .min_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h)))
        .unwrap();
    assert!(card.h >= 120.0 && card.w >= app.text_w - 20.0 && card.w <= app.text_w, "the picture's box, {card:?} in {}", app.text_w);
    let pics: Vec<u64> = blocks(&app).iter().filter(|b| matches!(b.kind, Kind::Image { .. })).map(|b| b.id).collect();
    let ours = app.row_rects.iter().find(|(b, _)| *b == pics[1]).map(|(_, r)| *r).unwrap();
    assert!(ours.h < 100.0, "{ours:?}");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    drop(app);
    remove_test_dir(dir);
    let _ = std::fs::remove_dir_all(&data);
}

/// A device joining an account lets its untouched welcome note go, and
/// once the account's notes are all here, puts it back only if the
/// account has none of its own: the account has one welcome note, never
/// two, nor none because a device joined.
#[test]
fn a_joining_device_leaves_the_account_one_welcome_note() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let welcomes = |app: &Carnet| app.store.notes.iter().filter(|n| n.title == "Welcome to Carnet").count();
    for keep in [true, false] {
        let data = std::env::temp_dir().join(format!("carnet-app-welcome-{keep}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
            let _ = stopped.await;
        }));
        #[allow(clippy::type_complexity)]
        let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&mut [(&mut Carnet, &mut Drive)]) -> bool| {
            let t0 = Instant::now();
            while !done(apps) {
                assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
                for (app, d) in apps.iter_mut() {
                    d.frame(&mut **app);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let (mut a, mut da, dir_a) = fresh(&format!("welcome-a-{keep}"));
        a.sync_server = origin.clone();
        a.sync_how = 0;
        a.sync_code = invite.text();
        a.on_plain(Msg::SyncStart);
        until("A online", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
        type_in(&mut a, &mut da, "Plans");
        a.save_current();
        if !keep {
            // The account's welcome deleted for good before B comes.
            let i = a.store.notes.iter().position(|n| n.title == "Welcome to Carnet").unwrap();
            a.store.trash(i).unwrap();
            let t = a.store.trashed().into_iter().find(|t| t.title == "Welcome to Carnet").unwrap();
            a.store.delete_forever(&t).unwrap();
            a.current = a.store.notes.iter().position(|n| n.title == "Plans").unwrap();
            a.reorder();
            a.refresh_trash();
        }
        until("A caught up", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.caught_up() && x[0].0.sync.waiting() == 0);
        a.on_plain(Msg::SyncPair);
        until("a code", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.code.is_some());
        let (mut b, mut db, dir_b) = fresh(&format!("welcome-b-{keep}"));
        assert_eq!(welcomes(&b), 1, "B's own, before it joins");
        b.sync_server = origin.clone();
        b.sync_code = a.sync.code.clone().unwrap();
        b.on_plain(Msg::SyncStart);
        until("B has the account's notes", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
            x[1].0.sync.caught_up() && x[1].0.store.notes.iter().any(|n| n.title == "Plans") && !x[1].0.sync.welcome_dropped
        });
        if keep {
            for _ in 0..20 {
                da.frame(&mut a);
                db.frame(&mut b);
            }
            assert_eq!((welcomes(&a), welcomes(&b)), (1, 1), "the account's own, on both");
        } else {
            assert_eq!(welcomes(&b), 1, "B's, back: the account had none");
            until("A has it too", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| welcomes(x[0].0) == 1);
        }
        let _ = stop.send(());
        let _ = rt.block_on(served);
        drop((a, b));
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
        let _ = std::fs::remove_dir_all(&data);
    }
}

/// A note come from another device while its picture is still on its
/// way (the found: "Picture not found" until it came) shows a quiet box
/// saying it's downloading, the picture's place; then the picture once
/// it lands. One that's nowhere is still "Picture not found".
#[test]
fn a_picture_on_its_way_by_sync_says_so_then_shows() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-coming-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    #[allow(clippy::type_complexity)]
    let until = |what: &str, apps: &mut [(&mut Carnet, &mut Drive)], done: &dyn Fn(&mut [(&mut Carnet, &mut Drive)]) -> bool| {
        let t0 = Instant::now();
        while !done(apps) {
            assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
            for (app, d) in apps.iter_mut() {
                d.frame(&mut **app);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let shown = |d: &Drive, s: &str| d.core.nodes().iter().any(|n| n.text.as_deref() == Some(s));

    // A: a note with a picture of Carnet's, on the server.
    let (mut a, mut da, dir_a) = fresh("coming-a");
    a.sync_server = origin.clone();
    a.sync_how = 0;
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("A online", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.status == crate::sync::Status::Online);
    type_in(&mut a, &mut da, "Harbour");
    key(&mut a, &mut da, "enter");
    let rgba: Vec<u8> = (0..400 * 300).flat_map(|_| [90u8, 140, 170, 255]).collect();
    a.paste_picture(400, 300, &rgba);
    type_in(&mut a, &mut da, "The boats");
    a.save_current();
    until("A's picture on the server", &mut [(&mut a, &mut da)], &|x| {
        x[0].0.store.replica.as_ref().is_some_and(|r| !r.vault().files().is_empty()) && x[0].0.sync.files_waiting() == 0
    });
    a.on_plain(Msg::SyncPair);
    until("a code", &mut [(&mut a, &mut da)], &|x| x[0].0.sync.code.is_some());

    // B comes in, its pictures held on the way.
    let (mut b, mut db, dir_b) = fresh("coming-b");
    b.sync.hold_fetches = true;
    b.sync_server = origin.clone();
    b.sync_code = a.sync.code.clone().unwrap();
    b.on_plain(Msg::SyncStart);
    until("B has the note, its picture asked for", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        x[1].0.store.notes.iter().any(|n| n.title == "Harbour") && x[1].0.store.replica.as_ref().is_some_and(|r| !r.vault().files().is_empty())
    });
    let i = b.store.notes.iter().position(|n| n.title == "Harbour").unwrap();
    b.open(i);
    db.core.set_inspect(true);
    until("B says it's on its way", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| shown(x[1].1, super::page_view::DOWNLOADING));
    assert!(!shown(&db, "Picture not found"));
    // In the box the picture takes: a picture's height at least.
    let words = db.core.nodes().iter().find(|n| n.text.as_deref() == Some(super::page_view::DOWNLOADING)).unwrap().rect;
    let card = db
        .core
        .nodes()
        .iter()
        .map(|n| n.rect)
        .filter(|r| r.x <= words.x && r.y <= words.y && r.x + r.w >= words.x + words.w && r.y + r.h >= words.y + words.h && r.h >= 100.0)
        .min_by(|p, q| (p.w * p.h).total_cmp(&(q.w * q.h)));
    assert!(card.is_some_and(|c| c.h >= 120.0), "{card:?}");
    // As tall as a picture its width (the finding: 120px tall, where the
    // picture came 456px, and the page jumped).
    let card = db
        .core
        .nodes()
        .iter()
        .map(|n| n.rect)
        .filter(|r| r.x <= words.x && r.y <= words.y && r.x + r.w >= words.x + words.w && r.y + r.h >= words.y + words.h && r.w >= 300.0)
        .min_by(|p, q| (p.w * p.h).total_cmp(&(q.w * q.h)))
        .unwrap();
    assert!((card.h - card.w * super::page_view::COMING_SHAPE).abs() <= 2.0, "{card:?}");

    // Let through: the picture.
    b.sync.hold_fetches = false;
    until("B shows the picture", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| {
        let (app, d) = (&mut *x[1].0, &mut *x[1].1);
        !shown(d, super::page_view::DOWNLOADING)
            && app.store.notes[app.current].doc.blocks().iter().any(|bl| match &bl.kind {
                Kind::Image { src, wiki, .. } => {
                    let f = app.page_folder();
                    app.store.picture_file(&f, src, *wiki).is_some_and(|p| app.images.is_ready(&p))
                }
                _ => false,
            })
    });
    assert!(!shown(&db, "Picture not found"));

    // One the vault doesn't name, nowhere at all: not found, as ever.
    let n = b.store.notes.iter().position(|n| n.title == "Harbour").unwrap();
    b.store.notes[n].doc.push(Block::new(Kind::Image { src: "attachments/nowhere.png".into(), alt: String::new(), width: None, float: Float::None, wiki: false }));
    b.store.notes[n].touch();
    until("B says the other is nowhere", &mut [(&mut a, &mut da), (&mut b, &mut db)], &|x| shown(x[1].1, "Picture not found"));

    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    drop((a, b));
    remove_test_dir(dir_a);
    remove_test_dir(dir_b);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn the_highlighter_and_code_shortcuts_take_off_what_they_put_on() {
    let (mut app, mut d, dir) = body("toggles");
    type_in(&mut app, &mut d, "some words here");
    let slot = app.caret.head.slot;
    // A word chosen with the space after it, as a drag often does: the
    // second press takes the style off again, with ⌘⇧H, ⌘E and ⌘⇧E.
    for (k, m) in [("h", CTRL_SHIFT), ("e", CTRL), ("e", CTRL_SHIFT)] {
        for (a, b) in [(0, 5), (4, 10), (0, 15)] {
            app.caret = Caret { anchor: Pos { slot, byte: a }, head: Pos { slot, byte: b } };
            d.frame(&mut app);
            key_with(&mut app, &mut d, k, m);
            assert_ne!(texts(&app)[0], "some words here", "{k} {a}..{b} puts it on");
            key_with(&mut app, &mut d, k, m);
            assert_eq!(texts(&app)[0], "some words here", "{k} {a}..{b} takes it off");
        }
    }
    // Nothing selected: ⌘⇧H twice is nothing at all…
    key(&mut app, &mut d, "end");
    key_with(&mut app, &mut d, "h", CTRL_SHIFT);
    key_with(&mut app, &mut d, "h", CTRL_SHIFT);
    type_in(&mut app, &mut d, " plain");
    assert_eq!(texts(&app)[0], "some words here plain");
    // …once is a highlighter for what is typed…
    key_with(&mut app, &mut d, "h", CTRL_SHIFT);
    type_in(&mut app, &mut d, " lit");
    assert_eq!(texts(&app)[0], "some words here plain ==lit==");
    // …and inside highlighted words it switches it off.
    key_with(&mut app, &mut d, "h", CTRL_SHIFT);
    type_in(&mut app, &mut d, " off");
    assert_eq!(texts(&app)[0], "some words here plain ==lit== off");
    let _ = std::fs::remove_dir_all(dir);
}

/// Each drawn row of `slot`, top to bottom: its top and the x its first
/// place is at, by every byte's caret after the first.
fn drawn_rows(app: &Carnet, d: &Drive, slot: Slot, len: usize) -> Vec<(f32, f32)> {
    let mut rows: Vec<(f32, f32)> = Vec::new();
    for b in 1..=len {
        let r = app.rect_at(&d.core, slot, b).unwrap();
        match rows.last_mut() {
            Some(last) if (last.0 - r.y).abs() < 0.5 => last.1 = last.1.min(r.x),
            _ => rows.push((r.y, r.x)),
        }
    }
    rows
}

/// A paragraph's first letter dropped (the toolbar's toggle) opens it
/// three lines tall, the lines beside it moved in past it and the
/// rest back at the page's left — and the paragraph is written as any
/// other: a caret before the letter as tall as it, typing there or
/// deleting the letter drops the next one, a click on it lands before or
/// after it, a selection reaches over it. The note keeps plain words.
#[test]
fn the_first_letter_drops_three_lines_and_the_paragraph_is_written_as_ever() {
    let (mut app, mut d, dir) = body("drop-cap");
    let words = "Once upon a time there was a lighthouse on a rock at the edge of the map, and a keeper who \
                 wrote down every ship that passed, one line each, with the hour and the colour of the sea, \
                 for years and years and years.";
    type_in(&mut app, &mut d, words);
    app.page_action("initial");
    let slot = app.caret.head.slot;
    d_frame_twice(&mut app, &mut d);
    let at = |app: &Carnet, d: &Drive, b: usize| app.rect_at(&d.core, slot, b).unwrap();
    let (o, after) = (at(&app, &d, 0), at(&app, &d, 1));
    let lh = after.h;
    assert!((o.h - 3.0 * lh).abs() < 1.0, "the letter's caret is three lines: {o:?}, a line {lh}");
    assert!(after.x > o.x + 20.0 && after.y == o.y, "the first line starts past it: {o:?} {after:?}");
    let rows = drawn_rows(&app, &d, slot, words.len());
    assert!(rows.len() >= 4, "the paragraph wraps past the letter: {rows:?}");
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r.1, if i < 3 { after.x } else { o.x }, "row {i}: {rows:?}");
    }
    assert_eq!(texts(&app)[0], words, "display only");

    // Typed before it, a letter is dropped in its place; taken back, and
    // the letter deleted, the next one is.
    app.caret = Caret::at(Pos { slot, byte: 0 });
    d_frame_twice(&mut app, &mut d);
    type_in(&mut app, &mut d, "W");
    assert_eq!(texts(&app)[0], format!("W{words}"));
    d_frame_twice(&mut app, &mut d);
    assert!((at(&app, &d, 0).h - 3.0 * lh).abs() < 1.0);
    key(&mut app, &mut d, "backspace");
    key(&mut app, &mut d, "delete");
    assert_eq!(texts(&app)[0], &words[1..]);
    d_frame_twice(&mut app, &mut d);
    assert!((at(&app, &d, 0).h - 3.0 * lh).abs() < 1.0, "the n is dropped now");
    type_in(&mut app, &mut d, "O");
    assert_eq!(texts(&app)[0], words);
    d_frame_twice(&mut app, &mut d);

    // A click on the letter's left half is before it, on its right half
    // after it, on whichever line it is beside; one under it is on the
    // fourth row's start.
    let o = at(&app, &d, 0);
    for row in [0.5, 2.5] {
        d.click(&mut app, o.x + 2.0, o.y + row * lh);
        d_frame_twice(&mut app, &mut d);
        assert_eq!((app.caret.head.slot, app.caret.head.byte), (slot, 0), "row {row}");
    }
    d.click(&mut app, (o.x + after.x) / 2.0 + 2.0, o.y + lh);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.caret.head.byte, 1);
    d.click(&mut app, o.x + 2.0, o.y + 3.5 * lh);
    d_frame_twice(&mut app, &mut d);
    let fourth = at(&app, &d, app.caret.head.byte);
    assert_eq!((fourth.x, fourth.y), (o.x, o.y + 3.0 * lh));
    // ↑ from there is the letter's line again: before it.
    key(&mut app, &mut d, "up");
    assert_eq!(app.caret.head.byte, 0);
    // A selection over it and on: the letter and the words beside it.
    for _ in 0..4 {
        key_with(&mut app, &mut d, "right", SHIFT);
    }
    assert_eq!((app.caret.anchor.byte, app.caret.head.byte), (0, 4));

    // Taken back: no drop, every row at the page's left.
    app.page_action("initial");
    assert!(!blocks(&app)[0].initial);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(at(&app, &d, 0).h, lh);
    let rows = drawn_rows(&app, &d, slot, words.len());
    assert!(rows[1..].iter().all(|r| r.1 == o.x), "{rows:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Any paragraph drops its first letter from the toolbar (or ⌥⌘8, or the
/// page's menu), and only one that asks: the note's first paragraph is
/// as any other. As tall as the lines it fills. The file keeps it as a tag around the
/// letter, which the note reads back; the letter coloured from the
/// toolbar is written with its colour in that tag, and drops in it.
#[test]
fn any_paragraph_drops_its_first_letter_from_the_toolbar() {
    let (mut app, mut d, dir) = body("drop-any");
    d.core.set_inspect(true);
    let words = "Once upon a time there was a lighthouse on a rock at the edge of the map, and a keeper who \
                 wrote down every ship that passed, one line each, with the hour and the colour of the sea, \
                 for years and years and years.";
    type_in(&mut app, &mut d, "A first paragraph, as it was.");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, words);
    let slot = app.caret.head.slot;
    d_frame_twice(&mut app, &mut d);
    let at = |app: &Carnet, d: &Drive, b: usize| app.rect_at(&d.core, slot, b).unwrap();
    let lh = at(&app, &d, 1).h;
    assert_eq!(at(&app, &d, 0).h, lh, "not dropped yet");
    let tool = d.key_of("t-initial").expect("the toolbar's Drop the first letter");
    d.click_key(&mut app, tool);
    d_frame_twice(&mut app, &mut d);
    assert!(blocks(&app)[1].initial && !blocks(&app)[0].initial);
    let (o, after) = (at(&app, &d, 0), at(&app, &d, 1));
    assert!((o.h - 3.0 * lh).abs() < 1.0, "three lines: {o:?}, a line {lh}");
    assert!(after.x > o.x + 20.0);
    let first = app.doc().blocks()[0].id;
    let r = app.rect_at(&d.core, Slot::block(first), 0).unwrap();
    assert_eq!(r.h, lh, "the note's first paragraph as it was");
    assert_eq!(texts(&app)[1], words, "the words as they were");
    let file = |app: &Carnet| std::fs::read_to_string(app.store.notes[app.current].path.clone().unwrap()).unwrap();
    app.store.notes[app.current].title = "Drops".into();
    app.save_current();
    let md = file(&app);
    assert!(md.contains("\n\n<span class=\"initial\">O</span>nce upon a time"), "{md}");
    let (_, back) = carnet_core::doc::from_markdown(&md);
    assert!(back.blocks()[1].initial && back.blocks()[1].text_ref() == Some(words));

    // The letter given a colour from the toolbar: in the tag, and its own.
    app.caret = Caret { anchor: Pos { slot, byte: 0 }, head: Pos { slot, byte: 1 } };
    app.on_plain(Msg::Tint { ink: 1, color: 0xC0553FFF });
    d_frame_twice(&mut app, &mut d);
    assert_eq!(texts(&app)[1], format!("<span style=\"color: #C0553F\">O</span>{}", &words[1..]));
    assert!((at(&app, &d, 0).h - 3.0 * lh).abs() < 1.0, "still dropped");
    app.save_current();
    assert!(file(&app).contains("<span class=\"initial\" style=\"color: #C0553F\">O</span>nce"), "{}", file(&app));

    // ⌥⌘8 (Ctrl+Alt+8) takes it off again; the page's menu puts it back.
    app.caret = Caret::at(Pos { slot, byte: 5 });
    key_with(&mut app, &mut d, "8", CTRL_ALT);
    assert!(!blocks(&app)[1].initial);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(at(&app, &d, 0).h, lh);
    let item = app.page_menu().into_iter().find(|m| m.label == "Drop the first letter").expect("in the page's menu");
    assert!(item.enabled);
    app.page_action("initial");
    assert!(blocks(&app)[1].initial);
    // On a heading there is none to drop: the tool is off.
    app.caret = Caret::at(Pos { slot: Slot::block(first), byte: 0 });
    app.on_plain(Msg::Turn { to: TURN_H2 });
    d_frame_twice(&mut app, &mut d);
    assert_eq!(page::initial_at(app.doc(), app.caret), None, "nothing to drop in a heading");
    assert!(!app.page_menu().into_iter().find(|m| m.label == "Drop the first letter").unwrap().enabled);
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A short first paragraph gets a short letter: set to drop four lines,
/// a sentence of one line drops none and one of two lines drops two, so
/// no tall letter stands beside a line or two of words.
#[test]
fn the_first_letter_drops_no_more_lines_than_the_paragraph_fills() {
    let (mut app, mut d, dir) = body("drop-cap-short");
    app.on_settings(Msg::SetInitialLines { to: 2 });
    assert_eq!(app.settings.initial_lines, 4);
    type_in(&mut app, &mut d, "One short line.");
    app.page_action("initial");
    let slot = app.caret.head.slot;
    d_frame_twice(&mut app, &mut d);
    let at = |app: &Carnet, d: &Drive, b: usize| app.rect_at(&d.core, slot, b).unwrap();
    let (o, after) = (at(&app, &d, 0), at(&app, &d, 1));
    assert_eq!(o.h, after.h, "one line: no drop");
    assert!(after.x - o.x < 20.0, "the letter at its own size: {o:?} {after:?}");

    // Two lines' worth at the page's width: a letter two lines tall.
    key(&mut app, &mut d, "end");
    type_in(&mut app, &mut d, " And then a few more words, enough of them to wrap onto a second line of the page here.");
    d_frame_twice(&mut app, &mut d);
    let (o, after) = (at(&app, &d, 0), at(&app, &d, 1));
    let lh = after.h;
    assert!((o.h - 2.0 * lh).abs() < 1.0, "two lines: {o:?}, a line {lh}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The dropped letter's styles: two lines tall or four, the caret before
/// it as tall and as many rows moved in past it, the rest at the page's
/// left; a letter font chosen from the list is kept in the settings and
/// the letter drawn in it. Its rows show in the settings, under "Dropped
/// letters", with no switch to drop every note's first.
#[test]
fn the_first_letter_drops_as_many_lines_as_chosen_in_the_font_chosen() {
    let (mut app, mut d, dir) = body("drop-cap-styles");
    let words = "In the old days there was a lighthouse on a rock at the edge of the map, and a keeper who \
                 wrote down every ship that passed, one line each, with the hour and the colour of the sea, \
                 for years and years and years, until the book was full and another came on the boat with \
                 the bread and the oil, and then another, and the shelf by the stove was all books.";
    type_in(&mut app, &mut d, words);
    app.page_action("initial");
    let slot = app.caret.head.slot;
    let at = |app: &Carnet, d: &Drive, b: usize| app.rect_at(&d.core, slot, b).unwrap();
    let mut wide = 0.0;
    for (to, n) in [(0, 2), (2, 4)] {
        app.on_settings(Msg::SetInitialLines { to });
        assert_eq!(app.settings.initial_lines, n as u32);
        d_frame_twice(&mut app, &mut d);
        let (o, after) = (at(&app, &d, 0), at(&app, &d, 1));
        let lh = after.h;
        assert!((o.h - n as f32 * lh).abs() < 1.0, "{n} lines: {o:?}, a line {lh}");
        let rows = drawn_rows(&app, &d, slot, words.len());
        assert!(rows.len() > n, "the paragraph wraps past the letter: {rows:?}");
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r.1, if i < n { after.x } else { o.x }, "{n} lines, row {i}: {rows:?}");
        }
        wide = after.x - o.x;
    }

    // The letters' own rows, and no Plain / Dropped switch over them.
    app.settings_open = true;
    assert!(drawn(&mut d, &mut app, "Dropped letters"));
    assert!(drawn(&mut d, &mut app, "Up to") && drawn(&mut d, &mut app, "Colour"));
    assert!(!drawn(&mut d, &mut app, "First letters") && d.key_of("initial").is_none(), "no switch for every note's first");
    assert!(drawn(&mut d, &mut app, "Same as headings"), "no font chosen: the headings'");
    app.on_settings(Msg::FontMenu { which: 2 });
    app.font_query = "inter".into();
    d.frame(&mut app);
    let i = app.font_rows.iter().position(|f| f == "Inter").expect("the shipped face is listed");
    app.on_settings(Msg::SetFont { which: 2, index: i as u64 });
    assert_eq!((app.settings.initial_font.as_str(), app.font_pick), ("Inter", None));
    assert_eq!((app.settings.font.as_str(), app.settings.heading_font.as_str()), ("", ""), "the others as they were");
    let back = Settings::parse(&app.settings.to_text());
    assert_eq!((back.initial_font.as_str(), back.initial_lines, back.initial_accent), ("Inter", 4, true));
    app.on_settings(Msg::SetInitialColor { to: 1 });
    assert!(!Settings::parse(&app.settings.to_text()).initial_accent);
    app.settings_open = false;
    d_frame_twice(&mut app, &mut d);
    let (o, after) = (at(&app, &d, 0), at(&app, &d, 1));
    assert!((o.h - 4.0 * after.h).abs() < 1.0);
    // An I: Inter's a bare stroke, the serif's with feet.
    assert!((after.x - o.x - wide).abs() > 1.0, "the I in another face: {} then {}", wide, after.x - o.x);
    assert_eq!(texts(&app)[0], words, "display only");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tab_in_text_is_a_tab_and_in_a_list_moves_the_item() {
    let (mut app, mut d, dir) = body("tab-char");
    type_in(&mut app, &mut d, "left right");
    let slot = app.caret.head.slot;
    d.frame(&mut app);
    // In the middle of the line: a tab character, never spaces, and the
    // words after it pushed along.
    app.caret = Caret::at(Pos { slot, byte: 4 });
    d_frame_twice(&mut app, &mut d);
    let before = app.rect_at(&d.core, slot, 5).map(|r| r.x).unwrap();
    key(&mut app, &mut d, "tab");
    assert_eq!(texts(&app)[0], "left\t right");
    assert!(page_focused(&app, &d), "the page keeps the keyboard");
    d_frame_twice(&mut app, &mut d);
    let after = app.rect_at(&d.core, slot, 6).map(|r| r.x).unwrap();
    assert!(after > before + 8.0, "the tab takes room: {before} → {after}");
    // Shift+Tab takes it back.
    key_with(&mut app, &mut d, "tab", SHIFT);
    assert_eq!(texts(&app)[0], "left right");
    // In a list item, wherever the caret is, the item goes in and out.
    key(&mut app, &mut d, "end");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "- item");
    key(&mut app, &mut d, "left");
    key(&mut app, &mut d, "left");
    key(&mut app, &mut d, "tab");
    assert!(matches!(&blocks(&app)[1].kind, Kind::Item { indent: 1, text, .. } if text == "item"));
    key_with(&mut app, &mut d, "tab", SHIFT);
    assert!(matches!(&blocks(&app)[1].kind, Kind::Item { indent: 0, .. }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_page_follows_the_text_size_down_to_12px() {
    let (mut app, mut d, dir) = body("small-text");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "- one");
    key(&mut app, &mut d, "enter");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "---");
    type_in(&mut app, &mut d, "after");
    let row = |app: &Carnet, i: usize| app.row_rects.iter().find(|(id, _)| *id == app.doc().blocks()[i].id).map(|(_, r)| *r).unwrap();
    d_frame_twice(&mut app, &mut d);
    let (item16, rule16) = (row(&app, 0), row(&app, 1));
    // The smallest step is 12px.
    for _ in 0..8 {
        app.on_settings(Msg::SetSize { by: -1 });
    }
    assert_eq!(app.settings.size, 12);
    d_frame_twice(&mut app, &mut d);
    let (item12, rule12) = (row(&app, 0), row(&app, 1));
    assert!(rule12.h < rule16.h * 0.85, "the divider's room shrinks: {} → {}", rule16.h, rule12.h);
    assert!(item12.h < item16.h * 0.85, "and a list item's: {} → {}", item16.h, item12.h);
    // Each way of drawing a divider, and a dropped first letter, draw:
    // words of your own not typed yet as the dots; a picture of your own,
    // none picked, asked for and not chosen until it is.
    app.page_action("initial");
    for to in 0..crate::settings::Divider::ALL.len() as u64 {
        app.on_settings(Msg::SetDivider { to });
        d_frame_twice(&mut app, &mut d);
        assert!(row(&app, 1).h > 10.0);
    }
    assert_eq!(app.settings.divider, crate::settings::Divider::Text);
    assert!(blocks(&app)[2].initial);
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

use super::page_view::DividerLook;

/// A note with a divider between two paragraphs, its settings kept in a
/// folder of their own (where a divider's picture is copied to).
fn divided(name: &str) -> (Carnet, Drive, std::path::PathBuf, std::path::PathBuf) {
    let (mut app, mut d, dir) = body(name);
    let config = dir.with_extension("config");
    let _ = std::fs::remove_dir_all(&config);
    app.settings.file = Some(config.join("settings.txt"));
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "above");
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "---");
    type_in(&mut app, &mut d, "below");
    assert!(matches!(blocks(&app)[1].kind, Kind::Divider));
    (app, d, dir, config)
}

/// A settings file from when there were nine ways to draw a divider:
/// one of the four gone opens as the nearest still offered, and the page
/// draws it, the note's `---` as it was.
#[test]
fn a_settings_file_naming_a_divider_gone_still_opens() {
    use crate::settings::Divider;
    let (store, dir) = temp_store("divider-old");
    let old = Settings::parse("theme = light\ndivider = flourish\nsize = 18\n");
    assert_eq!((old.divider, old.size), (Divider::Wave, 18));
    let mut app = Carnet::with_settings(store, old);
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    app.new_note();
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "---");
    type_in(&mut app, &mut d, "after");
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.divider_look(1.0, 600.0), DividerLook::Ornament(Divider::Wave));
    for (word, now) in [("stars", Divider::Dots), ("asterism", Divider::Dots), ("fleuron", Divider::Lozenge)] {
        app.settings = Settings::parse(&format!("divider = {word}\n"));
        d_frame_twice(&mut app, &mut d);
        assert_eq!(app.divider_look(1.0, 600.0), DividerLook::Ornament(now), "{word}");
    }
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Words of your own for dividers: chosen in Settings, typed in the field
/// that opens under the choices (the keyboard already in it), written to
/// the settings file as they are typed and read back at the next start;
/// drawn where the note's divider is, the note still saying `---`.
#[test]
fn a_dividers_own_words_are_typed_in_settings_and_kept_in_the_file() {
    use crate::settings::Divider;
    let (mut app, mut d, dir, config) = divided("divider-words");
    app.on_settings(Msg::ShowSettings);
    d_frame_twice(&mut app, &mut d);
    // The choices in the list's order, the words' among them.
    let at = Divider::ALL.iter().position(|d| *d == Divider::Text).unwrap();
    let tile = |d: &mut Drive| {
        let list = d.key_of("dividers").expect("the dividers' choices");
        d.core.nodes().iter().filter(|n| n.parent == Some(list)).nth(at).map(|n| n.key).expect("the words' choice")
    };
    for _ in 0..20 {
        let k = tile(&mut d);
        if d.rect_of(k).is_some_and(|r| r.y > 60.0 && r.y < 500.0) {
            break;
        }
        d.wheel(&mut app, 700.0, 400.0, 0.0, -120.0);
        d.frame(&mut app);
    }
    let k = tile(&mut d);
    d.click_key(&mut app, k);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.divider, Divider::Text);
    let focused = d.core.focus().and_then(|k| app.fields.get(&k)).copied();
    assert_eq!(focused, Some(Field::DividerText), "the keyboard where the words go");
    d.text(&mut app, "~ \u{2726} ~");
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.divider_text, "~ \u{2726} ~");
    let text = std::fs::read_to_string(config.join("settings.txt")).unwrap();
    assert!(text.contains("\ndivider = text\n") && text.contains("\ndivider_text = ~ \u{2726} ~\n"), "{text}");
    // Back on the page: the words where the divider is.
    app.on_settings(Msg::ShowSettings);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.divider_look(1.0, 600.0), DividerLook::Words("~ \u{2726} ~".into()));
    assert!(d.core.nodes().iter().any(|n| n.text.as_deref() == Some("~ \u{2726} ~")), "drawn on the page");
    assert!(app.note().markdown().contains("\n---\n"), "the file says ---");
    // The next start: read back.
    let back = Settings { file: Some(config.join("settings.txt")), ..Settings::parse(&text) };
    assert_eq!((back.divider, back.divider_text.as_str()), (Divider::Text, "~ \u{2726} ~"));
    // Emptied: the dots again, the choice kept.
    app.set_divider_text("");
    assert_eq!(app.divider_look(1.0, 600.0), DividerLook::Ornament(Divider::Dots));
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(config);
}

/// A picture of your own for dividers: asked for when chosen with none
/// picked, copied beside the settings file once picked, so the one picked
/// can go; drawn a line tall, its shape kept, never wider than the text.
/// Another picked, the last copy goes. The copy gone too, by the next
/// start: drawn as the dots, and Settings says why.
#[test]
fn a_dividers_picture_is_kept_beside_the_settings_and_falls_back_when_gone() {
    use crate::settings::Divider;
    let (mut app, mut d, dir, config) = divided("divider-picture");
    let at = Divider::ALL.iter().position(|d| *d == Divider::Picture).unwrap() as u64;
    app.on_settings(Msg::SetDivider { to: at });
    assert!(app.pick_divider_picture, "a picture asked for");
    assert_eq!(app.settings.divider, Divider::Dots, "not chosen until there is one");
    d.frame(&mut app);
    assert!(!app.pick_divider_picture);
    // A wide one, 400 × 40, from somewhere else.
    let picked = dir.with_extension("picked.png");
    std::fs::write(&picked, crate::images::encode_png(400, 40, &vec![200; 400 * 40 * 4]).unwrap()).unwrap();
    app.set_divider_picture(&picked);
    assert_eq!(app.settings.divider, Divider::Picture);
    let kept = app.settings.divider_picture_file().expect("kept");
    assert!(kept.starts_with(&config) && kept.exists(), "{kept:?}");
    std::fs::remove_file(&picked).unwrap();
    d_frame_twice(&mut app, &mut d);
    d_frame_twice(&mut app, &mut d);
    assert!(matches!(app.divider_look(1.0, 600.0), DividerLook::Picture(Some(_), w, h) if (w, h) == (280.0, 28.0)), "{:?}", app.divider_look(1.0, 600.0));
    assert!(matches!(app.divider_look(1.0, 100.0), DividerLook::Picture(Some(_), w, h) if (w, h) == (100.0, 10.0)), "narrowed to the room");
    assert!(matches!(app.divider_look(1.5, 600.0), DividerLook::Picture(Some(_), _, 42.0)), "with the text's size");
    // Another: the first copy goes.
    let second = dir.with_extension("second.png");
    std::fs::write(&second, crate::images::encode_png(20, 20, &[90; 20 * 20 * 4]).unwrap()).unwrap();
    app.set_divider_picture(&second);
    let kept2 = app.settings.divider_picture_file().unwrap();
    assert!(kept2 != kept && kept2.exists() && !kept.exists());
    assert!(second.exists(), "the reader's own picture stays where it was");
    // Its copy gone; the next start.
    std::fs::remove_file(&kept2).unwrap();
    let text = std::fs::read_to_string(config.join("settings.txt")).unwrap();
    let store = Store::open(dir.clone()).unwrap();
    let mut app = Carnet::with_settings(store, Settings { file: Some(config.join("settings.txt")), ..Settings::parse(&text) });
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    app.settings_open = true;
    d_frame_twice(&mut app, &mut d);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.divider, Divider::Picture, "still chosen");
    assert_eq!(app.divider_look(1.0, 600.0), DividerLook::Ornament(Divider::Dots));
    let said = d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.starts_with("The picture isn't shown (not found)")));
    assert!(said, "Settings says why");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_file(second);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(config);
}

#[test]
fn a_window_360_wide_holds_the_note_the_list_and_the_settings() {
    let (store, dir) = temp_store("w360");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 360.0, 640.0).framing();
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    // Everything that can be pressed is inside the window.
    let outside = |d: &mut Drive| -> Vec<String> {
        d.core
            .nodes()
            .iter()
            .filter(|n| n.role.is_some_and(|r| matches!(r, Role::Button | Role::Checkbox)) && n.rect.w > 0.0)
            .filter(|n| n.rect.x < -0.5 || n.rect.x + n.rect.w > 360.5)
            .map(|n| format!("{:?} {:?} {:?}", n.label, n.text, n.rect))
            .collect()
    };
    for k in ["t-back", "t-pin", "t-trash"] {
        assert!(d.key_of(k).is_some(), "{k}");
    }
    assert_eq!(outside(&mut d), Vec::<String>::new(), "the note");
    let back = d.key_of("t-back").unwrap();
    d.click_key(&mut app, back);
    d_frame_twice(&mut app, &mut d);
    assert!(d.key_of("library").is_some());
    assert_eq!(outside(&mut d), Vec::<String>::new(), "the list");
    app.on_settings(Msg::ShowSettings);
    d_frame_twice(&mut app, &mut d);
    assert!(app.settings_open);
    assert_eq!(outside(&mut d), Vec::<String>::new(), "the settings");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_selection_held_past_the_page_edge_scrolls_it() {
    let (mut app, mut d, dir) = body("drag-scroll");
    for i in 0..60 {
        if i > 0 {
            key(&mut app, &mut d, "enter");
        }
        type_in(&mut app, &mut d, &format!("line {i}"));
    }
    let key = app.scroll_key.unwrap();
    d.core.set_scroll(key, Vec2::new(0.0, 0.0));
    for _ in 0..20 {
        d.advance(0.05);
        d.frame(&mut app);
    }
    let page_rect = app.page_rect.unwrap();
    assert!(d.core.scroll_offset(key).y < 1.0, "at the top");
    let first = app.row_rects[0].1;
    let from = Vec2::new(first.x + 20.0, first.y + first.h / 2.0);
    // Pressed in the first line, taken below the window, and held still.
    let below = Vec2::new(from.x, page_rect.y + page_rect.h + 40.0);
    d.input(&mut app, InputEvent::CursorMoved(from));
    d.input(&mut app, InputEvent::mouse_down(1));
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(from.x + 8.0, from.y + 8.0)));
    d.input(&mut app, InputEvent::CursorMoved(below));
    d.frame(&mut app);
    let mut offsets = Vec::new();
    for _ in 0..30 {
        d.advance(1.0 / 60.0);
        d.frame(&mut app);
        offsets.push(d.core.scroll_offset(key).y);
    }
    assert!(offsets[29] > offsets[5] + 50.0, "it goes on scrolling with the pointer still: {offsets:?}");
    // 40px past the edge (64 into the band): about 640 px a second.
    let per_frame = (offsets[29] - offsets[9]) / 20.0;
    assert!((6.0..16.0).contains(&per_frame), "{per_frame} px a frame");
    let head = app.doc().index_of(app.caret.head.slot.block).unwrap();
    assert!(head > 10, "the selection follows the words under the pointer: block {head}");
    assert_eq!(page::ordered(app.doc(), app.caret).0.slot.block, app.doc().blocks()[0].id, "from where it began");
    // Let go: it stops.
    d.input(&mut app, InputEvent::mouse_up());
    d.frame(&mut app);
    let stopped = d.core.scroll_offset(key).y;
    for _ in 0..5 {
        d.advance(1.0 / 60.0);
        d.frame(&mut app);
    }
    assert!((d.core.scroll_offset(key).y - stopped).abs() < 0.5, "and stops when let go");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn several_notes_and_folders_are_chosen_and_go_together() {
    let dir = std::env::temp_dir().join(format!("carnet-ui-pick-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Box")).unwrap();
    std::fs::create_dir_all(dir.join("Other")).unwrap();
    for (name, body) in [("One", "1"), ("Two", "2"), ("Three", "3"), ("Box/Inside", "in"), ("Other/Elsewhere", "e")] {
        let leaf = name.rsplit('/').next().unwrap();
        std::fs::write(dir.join(format!("{name}.md")), format!("# {leaf}\n\n{body}\n")).unwrap();
    }
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    app.on_settings(Msg::SetSort { to: 1 });
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    let title = |app: &Carnet, i: usize| app.store.notes[i].title.clone();
    let card = |app: &Carnet, t: &str| {
        let i = app.store.notes.iter().position(|n| n.title == t).unwrap();
        app.card_rects.iter().find(|(n, _)| *n == i).map(|(_, r)| r.center()).unwrap()
    };
    let click_with = |app: &mut Carnet, d: &mut Drive, at: Vec2, m: KeyMods| {
        d.input(app, InputEvent::Modifiers(m));
        d.click(app, at.x, at.y);
        d.input(app, InputEvent::Modifiers(KeyMods::NONE));
        d.frame(app);
    };
    let cmd = KeyMods { shift: false, ctrl: !cfg!(target_os = "macos"), alt: false, super_key: cfg!(target_os = "macos") };
    // A plain click opens; ⇧-click chooses everything from it to there.
    let at = card(&app, "Elsewhere");
    d.click(&mut app, at.x, at.y);
    assert_eq!(title(&app, app.current), "Elsewhere");
    let at = card(&app, "Three");
    click_with(&mut app, &mut d, at, SHIFT);
    let names = |app: &Carnet| {
        let mut v: Vec<String> = app
            .picked
            .iter()
            .map(|p| match p {
                Pick::Note(id) => app.store.notes.iter().find(|n| n.id == *id).unwrap().title.clone(),
                Pick::Folder(f) => format!("{f}/"),
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(names(&app), ["Elsewhere", "Inside", "One", "Three"], "by title: Elsewhere, Inside, One, Three");
    assert_eq!(title(&app, app.current), "Elsewhere", "the open note stays open");
    // ⌘-click takes one out, and adds a folder.
    let at = card(&app, "Inside");
    click_with(&mut app, &mut d, at, cmd);
    assert_eq!(names(&app), ["Elsewhere", "One", "Three"]);
    let other = app.folder_rects.iter().find(|(f, _)| f == "Other").map(|(_, r)| r.center()).unwrap();
    click_with(&mut app, &mut d, other, cmd);
    assert_eq!(names(&app), ["Elsewhere", "One", "Other/", "Three"]);
    // To the trash together (the note in the chosen folder goes with
    // it), and one Undo brings all of it back.
    key_with(&mut app, &mut d, "backspace", CTRL_SHIFT);
    assert!(app.picked.is_empty());
    assert!(!dir.join("Other").exists() && !dir.join("One.md").exists() && !dir.join("Three.md").exists());
    assert!(dir.join("Two.md").exists() && dir.join("Box/Inside.md").exists());
    let said = app.notice.as_ref().map(|n| n.text.clone()).unwrap();
    assert_eq!(said, "Moved a folder and 2 notes to Deleted");
    app.on_plain(Msg::Undo);
    d.frame(&mut app);
    assert!(dir.join("Other/Elsewhere.md").exists() && dir.join("One.md").exists() && dir.join("Three.md").exists());
    // Chosen cards dragged to a folder all go there.
    d_frame_twice(&mut app, &mut d);
    let at = card(&app, "One");
    d.click(&mut app, at.x, at.y);
    let at = card(&app, "Two");
    click_with(&mut app, &mut d, at, cmd);
    assert_eq!(names(&app), ["One", "Two"]);
    let menu: Vec<String> = app.picked_menu().iter().map(|m| m.label.to_string()).collect();
    assert!(menu.contains(&"Move 2 notes to".to_string()) && menu.contains(&"Move 2 notes to Deleted".to_string()), "{menu:?}");
    let into = app.folder_rects.iter().find(|(f, _)| f == "Box").map(|(_, r)| r.center()).unwrap();
    d.drag(&mut app, at, into);
    d.frame(&mut app);
    assert!(dir.join("Box/One.md").exists() && dir.join("Box/Two.md").exists());
    // Esc lets go of a choice.
    let at = card(&app, "Three");
    click_with(&mut app, &mut d, at, cmd);
    assert!(!app.picked.is_empty());
    d.core.set_focus(None);
    key(&mut app, &mut d, "escape");
    assert!(app.picked.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_library_is_as_wide_as_its_edge_is_dragged() {
    let (mut app, mut d, dir) = fresh("library-width");
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    let pane = |d: &mut Drive| d.key_of("library-pane").and_then(|k| d.rect_of(k)).map(|r| r.w).unwrap();
    assert_eq!(pane(&mut d), 268.0);
    let edge = d.key_of("library-edge").and_then(|k| d.rect_of(k)).expect("the library's edge");
    let at = Vec2::new(edge.x + 3.0, 300.0);
    d.drag(&mut app, at, Vec2::new(at.x + 80.0, 320.0));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.library_w, 348, "wider by as much as the drag");
    assert_eq!(pane(&mut d), 348.0);
    // Never past what the settings allow.
    let edge = d.key_of("library-edge").and_then(|k| d.rect_of(k)).unwrap();
    d.drag(&mut app, Vec2::new(edge.x + 3.0, 300.0), Vec2::new(1000.0, 300.0));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.library_w, 480);
    // A narrower library stays beside a narrower window.
    let edge = d.key_of("library-edge").and_then(|k| d.rect_of(k)).unwrap();
    d.drag(&mut app, Vec2::new(edge.x + 3.0, 300.0), Vec2::new(10.0, 300.0));
    assert_eq!(app.settings.library_w, 200);
    assert!(Settings::parse(&app.settings.to_text()).library_w == 200, "kept in the settings");
    // A double-click puts it back.
    let edge = d.key_of("library-edge").and_then(|k| d.rect_of(k)).unwrap();
    d.double_click(&mut app, edge.x + 3.0, 300.0);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.library_w, 268);
    let _ = std::fs::remove_dir_all(dir);
}

/// A drag never folds the library away: on a window with no room for a
/// wider one, the edge stops where the page keeps its least, every frame
/// of the drag; a width saved on a wider window is drawn as wide as this
/// one allows, and comes back on a wider one.
#[test]
fn the_library_edge_stops_where_the_page_would_be_too_narrow() {
    let (store, dir) = temp_store("library-edge-stops");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 900.0, 700.0).framing();
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    let pane = |d: &mut Drive| d.key_of("library-pane").and_then(|k| d.rect_of(k)).map(|r| r.w);
    let most = 900.0 - super::PAGE_MIN;
    assert_eq!(pane(&mut d), Some(268.0));
    let edge = d.key_of("library-edge").and_then(|k| d.rect_of(k)).unwrap();
    let x0 = edge.x + 3.0;
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(x0, 300.0)));
    d.input(&mut app, InputEvent::mouse_down(1));
    for dx in [20.0, 40.0, 60.0, 120.0, 300.0] {
        d.move_to(&mut app, x0 + dx, 300.0);
        d_frame_twice(&mut app, &mut d);
        let w = pane(&mut d).unwrap_or_else(|| panic!("the library still shows at +{dx}"));
        assert_eq!(w, (268.0 + dx).min(most), "+{dx}");
        assert!(d.key_of("t-back").is_none(), "not folded to a button at +{dx}");
    }
    d.input(&mut app, InputEvent::mouse_up());
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.library_w as f32, most, "saved as drawn");
    // Saved wider than this window allows: drawn as wide as it can be.
    app.settings.library_w = 480;
    d_frame_twice(&mut app, &mut d);
    assert_eq!(pane(&mut d), Some(most));
    // A wider window: as saved.
    d.viewport.w = 1300.0;
    d_frame_twice(&mut app, &mut d);
    assert_eq!(pane(&mut d), Some(480.0));
    // Narrower: from the width drawn, a drag narrows it at once.
    d.viewport.w = 900.0;
    d_frame_twice(&mut app, &mut d);
    let edge = d.key_of("library-edge").and_then(|k| d.rect_of(k)).unwrap();
    d.drag(&mut app, Vec2::new(edge.x + 3.0, 300.0), Vec2::new(edge.x + 3.0 - 30.0, 300.0));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.library_w as f32, most - 30.0);
    assert_eq!(pane(&mut d), Some(most - 30.0));
    // Only a window too narrow for the design's width (or a narrower
    // one dragged) folds it, as ever.
    d.viewport.w = 268.0 + super::PAGE_MIN;
    d_frame_twice(&mut app, &mut d);
    assert_eq!(pane(&mut d), Some(268.0));
    d.viewport.w = 268.0 + super::PAGE_MIN - 1.0;
    d_frame_twice(&mut app, &mut d);
    assert!(pane(&mut d).is_none() && d.key_of("t-back").is_some(), "folded");
    app.settings.library_w = 220;
    d_frame_twice(&mut app, &mut d);
    assert_eq!(pane(&mut d), Some(220.0), "dragged narrower, it stays beside the page");
    let _ = std::fs::remove_dir_all(dir);
}

/// The library's edge sits just past the column, clear of its lists'
/// scrollbars; its hint shows by the pointer once it has rested there,
/// never while the edge is dragged, and not again after a drag until the
/// pointer has left it.
#[test]
fn the_library_edge_keeps_off_the_scrollbar_and_hints_by_the_pointer() {
    let (mut app, mut d, dir) = fresh("library-edge-hint");
    // Enough notes for the library to scroll.
    for _ in 0..30 {
        app.new_note();
    }
    d_frame_twice(&mut app, &mut d);
    let rect = |d: &mut Drive, k: &str| d.key_of(k).and_then(|k| d.rect_of(k));
    let pane = rect(&mut d, "library-pane").unwrap();
    let list = rect(&mut d, "library").expect("the notes list");
    let edge = rect(&mut d, "library-edge").unwrap();
    assert!(edge.x >= pane.x + pane.w - 0.5, "past the column: {edge:?} {pane:?}");
    assert!(edge.x >= list.x + list.w - 0.5, "clear of the list and its scrollbar: {edge:?} {list:?}");
    let hint = |d: &mut Drive| rect(d, "library-edge-hint");

    // Rested on: nothing at once; after a moment, by the pointer.
    let at = Vec2::new(edge.x + 3.0, 300.0);
    d.move_to(&mut app, at.x, at.y);
    d_frame_twice(&mut app, &mut d);
    assert!(hint(&mut d).is_none(), "not at once");
    d.advance(0.6);
    d_frame_twice(&mut app, &mut d);
    let h = hint(&mut d).expect("after a moment");
    assert!((h.x - at.x).abs() < 20.0 && h.y > at.y && h.y - at.y < 30.0, "by the pointer: {h:?} {at:?}");
    // Near the window's foot: above the pointer.
    d.move_to(&mut app, at.x, d.viewport.h - 10.0);
    d_frame_twice(&mut app, &mut d);
    let h = hint(&mut d).expect("still on the edge");
    assert!(h.y + h.h <= d.viewport.h - 10.0, "above the pointer: {h:?}");

    // Dragged: gone, every frame of it, and after it while the pointer
    // stays on the edge.
    let edge = rect(&mut d, "library-edge").unwrap();
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(edge.x + 3.0, 300.0)));
    d.input(&mut app, InputEvent::mouse_down(1));
    for x in [10.0, 20.0, 30.0, 40.0] {
        d.move_to(&mut app, edge.x + 3.0 + x, 300.0);
        d.advance(0.3);
        d_frame_twice(&mut app, &mut d);
        assert!(hint(&mut d).is_none(), "while dragged, at +{x}");
    }
    d.input(&mut app, InputEvent::mouse_up());
    assert_eq!(app.settings.library_w, 268 + 40, "it was dragged");
    d.advance(1.0);
    d_frame_twice(&mut app, &mut d);
    assert!(hint(&mut d).is_none(), "not straight after the drag");
    // Off it and back: the hint again, after its moment.
    d.move_to(&mut app, 700.0, 300.0);
    d_frame_twice(&mut app, &mut d);
    let edge = rect(&mut d, "library-edge").unwrap();
    d.move_to(&mut app, edge.x + 3.0, 300.0);
    d_frame_twice(&mut app, &mut d);
    d.advance(0.6);
    d_frame_twice(&mut app, &mut d);
    assert!(hint(&mut d).is_some());
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A replica a newer Carnet wrote: the notes open from their files, with
/// no History, and the window says why.
#[test]
fn a_newer_replica_is_said_and_the_notes_still_open() {
    let (store, dir) = temp_store("newer-replica");
    drop(store);
    let replica = dir.with_extension("replica");
    std::fs::write(replica.join("version"), format!("{}\n", carnet_sync::LAYOUT + 1)).unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    assert_eq!(app.error.as_deref(), Some("These notes' history was written by a newer Carnet. Update Carnet to open it."));
    // (The inspector keeps a line's first 60 letters.)
    d.core.set_inspect(true);
    d.frame(&mut app);
    assert!(d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.starts_with("These notes' history was written by a newer Carnet."))));
    assert_eq!(app.store.notes[0].title, "Welcome to Carnet");
    assert!(app.store.notes[0].key.is_none(), "no History for it");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(replica);
}

/// A font list's first row, the face it falls back to, is chosen by a
/// click as any other: the letter back to the headings' face, the
/// headings back to the built-in one. (Its message's number once came
/// back from kui as another.)
#[test]
fn a_font_lists_default_row_is_chosen_by_a_click() {
    let (store, dir) = temp_store("font-default");
    let mut app = Carnet::new(store);
    // Tall enough for the lists without scrolling.
    let mut d = Drive::new(Core::new(), 1100.0, 1600.0).framing();
    app.settings.initial_font = "Inter".into();
    app.settings.heading_font = "Inter".into();
    app.settings_open = true;
    d.core.set_inspect(true);
    app.settings.font = "Georgia".into();
    for (which, name) in [(2, "Same as headings"), (1, "Source Serif 4 (built in)"), (0, "Inter (built in)")] {
        app.on_settings(Msg::FontMenu { which });
        d_frame_twice(&mut app, &mut d);
        let r = d.core.nodes().iter().find(|n| n.text.as_deref() == Some(name)).map(|n| n.rect).unwrap();
        d.click(&mut app, r.x + 20.0, r.y + r.h / 2.0);
        d.frame(&mut app);
        assert_eq!(app.font_pick, None, "{name}: the list closes");
    }
    assert_eq!((app.settings.initial_font.as_str(), app.settings.heading_font.as_str(), app.settings.font.as_str()), ("", "", ""));
    // And each pill says so.
    d_frame_twice(&mut app, &mut d);
    for (pill, name) in [("font-body", "Inter (built in)"), ("font-head", "Source Serif 4 (built in)"), ("font-initial", "Same as headings")] {
        assert_eq!(d.texts_under(pill), [name]);
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// The built-in face shows once in its list, as the default: installed
/// under its own name it is the same face, and a setting naming it is
/// the default. The other lists keep it by name.
#[test]
fn a_built_in_face_is_listed_once() {
    let (store, dir) = temp_store("font-once");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 1600.0).framing();
    app.settings.font = "Inter".into();
    app.settings_open = true;
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(d.texts_under("font-body"), ["Inter (built in)"], "named as the default");
    app.on_settings(Msg::FontMenu { which: 0 });
    d_frame_twice(&mut app, &mut d);
    assert!(!app.font_rows.iter().any(|f| f == "Inter"), "not again among the installed");
    assert!(app.font_rows.iter().any(|f| f == "Source Serif 4"), "the headings' face is a choice for text");
    let row = d.key_named("Inter (built in)").expect("the default row");
    let chosen = d.core.nodes().into_iter().find(|n| n.key == row);
    assert!(chosen.as_ref().is_some_and(|n| n.bg != Color::TRANSPARENT), "the default row is the one chosen: {chosen:?}");
    app.on_settings(Msg::FontMenu { which: 1 });
    d_frame_twice(&mut app, &mut d);
    assert!(!app.font_rows.iter().any(|f| f == "Source Serif 4"));
    assert!(app.font_rows.iter().any(|f| f == "Inter"));
    app.on_settings(Msg::FontMenu { which: 2 });
    d_frame_twice(&mut app, &mut d);
    assert!(app.font_rows.iter().any(|f| f == "Inter") && app.font_rows.iter().any(|f| f == "Source Serif 4"), "the letter's has both");
    // Searched for by its name, the default is what Enter takes.
    app.on_settings(Msg::FontMenu { which: 0 });
    app.settings.font = "Georgia".into();
    app.font_query = "inter".into();
    d_frame_twice(&mut app, &mut d);
    if app.font_rows.is_empty() {
        app.apply_typed_font();
        assert_eq!(app.settings.font, "");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Code is drawn in a monospaced face, a block of it and a word of it:
/// ten i's as wide as ten M's, where the text's face makes them far
/// apart. The face is the system's monospace until one is chosen; a
/// chosen one draws the code, and the text stays in its own.
#[test]
fn code_is_drawn_in_the_code_face() {
    let (store, dir) = temp_store("code-face");
    drop(store);
    let (i, m) = ("iiiiiiiiii", "MMMMMMMMMM");
    std::fs::write(dir.join("Code.md"), format!("# Code\n\n```\n{i}\n```\n\n```\n{m}\n```\n\n`{i}`\n\n`{m}`\n\n{i}\n\n{m}\n")).unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 900.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    let n = app.store.notes.iter().position(|n| n.title == "Code").unwrap();
    app.open(n);
    // The widths drawn, top to bottom: the blocks, the words, the text.
    let widths = |app: &mut Carnet, d: &mut Drive| -> Vec<f32> {
        for _ in 0..3 {
            d.frame(app);
        }
        let mut found: Vec<(f32, f32)> = d
            .core
            .nodes()
            .iter()
            .filter(|n| n.rect.x > 268.0 && (n.text.as_deref() == Some(i) || n.text.as_deref() == Some(m)))
            .map(|n| (n.rect.y, n.rect.w))
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|f| f.1).collect()
    };
    let w = widths(&mut app, &mut d);
    assert_eq!(w.len(), 6, "{w:?}");
    assert!((w[0] - w[1]).abs() < 0.5, "a block of code is monospaced: {w:?}");
    assert!((w[2] - w[3]).abs() < 0.5, "a word of code is monospaced: {w:?}");
    assert!(w[5] > w[4] * 1.5, "the text is not: {w:?}");
    // The system's monospace, at code's size (whole px, as the page has it).
    let style = TextStyle::new(Type::CODE.size.round()).family(kui_native::FontFamily::Mono);
    let mono = d.core.measure_text(m, &style, None).width;
    assert!((w[1] - mono).abs() < 0.5, "{} drawn, {mono} measured", w[1]);

    // Another monospaced face installed, one that draws wider or narrower,
    // from those code's list offers: chosen, it draws the code.
    let mut offered: Vec<String> = crate::look::code_families(&mut d.core).into_iter().collect();
    offered.sort();
    let other = offered.into_iter().find_map(|f| {
        let id = d.core.add_system_font(&f)?;
        let wide = d.core.measure_text(m, &style.family(kui_native::FontFamily::Custom(id)), None).width;
        ((wide - mono).abs() > 2.0).then_some((f, wide))
    });
    if let Some((name, wide)) = other {
        app.settings.code_font = name.clone();
        let w = widths(&mut app, &mut d);
        assert!((w[1] - wide).abs() < 0.5, "{name}: {} drawn, {wide} measured", w[1]);
        assert!((w[0] - w[1]).abs() < 0.5 && (w[2] - w[3]).abs() < 0.5, "{w:?}");
        assert!(w[5] > w[4] * 1.5, "the text in its own face still: {w:?}");
    } else {
        eprintln!("skipped: no second monospaced face installed");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Settings → Fonts → Code: its list holds the monospaced families only,
/// the system's own first; a click chooses one, its pill says so, and
/// Reset puts the system's back.
#[test]
fn the_code_font_is_chosen_from_monospaced_families() {
    let (store, dir) = temp_store("code-font");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 1600.0).framing();
    app.settings_open = true;
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(d.texts_under("font-code"), ["System monospace"]);
    app.on_settings(Msg::FontMenu { which: 3 });
    d_frame_twice(&mut app, &mut d);
    let mono: std::collections::HashSet<String> =
        d.core.system_fonts().into_iter().filter(|f| f.monospaced).map(|f| f.family).collect();
    assert!(app.font_rows.iter().all(|f| mono.contains(f)), "monospaced only: {:?}", app.font_rows);
    // And each sets letters so. A face can say it is fixed-pitch and have
    // no letters — Noto Color Emoji on Linux — and code chosen in it was
    // drawn in the proportional face they fell back to.
    for f in &app.font_rows {
        let id = d.core.add_system_font(f).unwrap();
        let style = TextStyle::new(16.0).family(kui_native::FontFamily::Custom(id));
        let i = d.core.measure_text("iiiiiiiiii", &style, None).width;
        let m = d.core.measure_text("MMMMMMMMMM", &style, None).width;
        assert!((i - m).abs() < 0.5, "{f}: ten i's {i}, ten M's {m}");
    }
    assert!(!app.font_rows.iter().any(|f| f == "Inter" || f == "Source Serif 4"));
    assert!(d.key_named("System monospace").is_some(), "the default row");
    let Some(name) = app.font_rows.first().cloned() else {
        eprintln!("skipped: no monospaced family installed");
        let _ = std::fs::remove_dir_all(dir);
        return;
    };
    let r = d.core.nodes().iter().find(|n| n.text.as_deref() == Some(name.as_str())).map(|n| n.rect).unwrap();
    d.click(&mut app, r.x + 20.0, r.y + r.h / 2.0);
    d_frame_twice(&mut app, &mut d);
    assert_eq!((app.settings.code_font.as_str(), app.font_pick), (name.as_str(), None));
    assert_eq!(d.texts_under("font-code"), [name.as_str()]);
    assert!(app.settings.to_text().contains(&format!("code_font = {name}\n")));
    // Searched, the list narrows as the others do.
    app.on_settings(Msg::FontMenu { which: 3 });
    app.font_query = "no such face".into();
    d_frame_twice(&mut app, &mut d);
    assert!(app.font_rows.is_empty());
    app.on_settings(Msg::FontMenu { which: 3 });
    // Reset: the system's again.
    app.on_settings(Msg::ResetLook);
    app.on_settings(Msg::ResetLook);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.code_font, "");
    assert_eq!(d.texts_under("font-code"), ["System monospace"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// A font list opens afresh: its search empty and every font listed,
/// whatever was searched for the last time.
#[test]
fn a_font_list_opens_with_its_search_empty() {
    let (store, dir) = temp_store("font-search");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 1600.0).framing();
    app.settings_open = true;
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    app.on_settings(Msg::FontMenu { which: 1 });
    d_frame_twice(&mut app, &mut d);
    let all = app.font_rows.len();
    let k = app_field(&app, Field::Font).expect("the search");
    d.focus(&mut app, k);
    d.text(&mut app, "inte");
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.font_query, "inte");
    assert!(app.font_rows.len() < all);
    // Chosen from the search: the list closes.
    key(&mut app, &mut d, "enter");
    assert_eq!(app.font_pick, None);
    app.on_settings(Msg::FontMenu { which: 1 });
    d_frame_twice(&mut app, &mut d);
    let k = app_field(&app, Field::Font).expect("the search again");
    assert_eq!(d.core.edit_text(k).as_deref(), Some(""), "the field empty");
    assert_eq!((app.font_query.as_str(), app.font_rows.len()), ("", all), "and the list whole");
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: Esc in an open font list closed all of Settings. Esc
/// closes the innermost thing open first — the list, the keyboard in its
/// search or not; a question asked (Cancel / Yes) — and only then
/// Settings. A font list opens with the keyboard in its search.
#[test]
fn escape_closes_a_font_list_before_settings() {
    let (store, dir) = temp_store("font-escape");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 1600.0).framing();
    app.settings_open = true;
    d_frame_twice(&mut app, &mut d);
    for which in [0, 1, 3] {
        app.on_settings(Msg::FontMenu { which });
        d_frame_twice(&mut app, &mut d);
        let k = app_field(&app, Field::Font).expect("the search");
        assert_eq!(d.core.focus(), Some(k), "the keyboard in the search ({which})");
        key(&mut app, &mut d, "escape");
        assert_eq!(app.font_pick, None, "the list closed");
        assert!(app.settings_open, "Settings still open");
    }
    // The search let go of first: Esc still closes the list alone.
    app.on_settings(Msg::FontMenu { which: 3 });
    d_frame_twice(&mut app, &mut d);
    d.focus(&mut app, Key::ROOT);
    d.frame(&mut app);
    key(&mut app, &mut d, "escape");
    assert_eq!((app.font_pick, app.settings_open), (None, true));
    // A question asked: Esc is its Cancel.
    app.on_settings(Msg::ResetLook);
    d_frame_twice(&mut app, &mut d);
    assert!(app.reset_armed);
    key(&mut app, &mut d, "escape");
    assert!(!app.reset_armed && app.settings_open);
    // Nothing open inside: Settings closes.
    key(&mut app, &mut d, "escape");
    assert!(!app.settings_open);
    let _ = std::fs::remove_dir_all(dir);
}

fn app_field(app: &Carnet, f: Field) -> Option<Key> {
    app.fields.iter().find(|(_, x)| **x == f).map(|(k, _)| *k)
}

/// The page is as wide as chosen whatever the text's size: smaller text
/// doesn't narrow it. A narrow window still holds it.
#[test]
fn the_page_keeps_its_width_whatever_the_text_size() {
    let (store, dir) = temp_store("page-width");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1600.0, 900.0).framing();
    let mut widths = Vec::new();
    for size in [12, 16, 24] {
        app.settings.size = size;
        d_frame_twice(&mut app, &mut d);
        widths.push(app.text_w);
    }
    assert!(widths.iter().all(|w| *w == widths[0]), "{widths:?}");
    app.on_settings(Msg::SetWidth { to: 2 });
    d_frame_twice(&mut app, &mut d);
    assert!(app.text_w > widths[0], "wide is wider");
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 360.0, 700.0).framing();
    for size in [12, 24] {
        app.settings.size = size;
        d_frame_twice(&mut app, &mut d);
        assert!(app.text_w > 280.0 && app.text_w < 360.0, "{size}: {}", app.text_w);
    }
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Settings → Library → Dates: the date over the title and on the cards
/// is when the note was made, read from its history where its file's is
/// later (a file saved before births were kept).
#[test]
fn the_date_shown_can_be_when_the_note_was_made() {
    let (mut app, mut d, dir) = body("dates");
    type_in(&mut app, &mut d, "words");
    app.save_current();
    let now = SystemTime::now();
    let note = app.current;
    app.store.notes[note].created = now - Duration::from_secs(3 * 86_400);
    let meta = |d: &mut Drive, app: &mut Carnet| {
        d.core.set_inspect(true);
        d.frame(app);
        d.core.nodes().iter().filter_map(|n| n.text.clone()).find(|t| t.contains("  \u{b7}  1 word")).unwrap_or_default()
    };
    assert!(meta(&mut d, &mut app).starts_with("just now"), "last edited, at first");
    app.on_plain(Msg::SetDates { to: 1 });
    assert!(app.settings.dates_created);
    assert!(Settings::parse(&app.settings.to_text()).dates_created, "kept");
    let m = meta(&mut d, &mut app);
    let created = app.store.notes[note].created;
    let day = super::chrome::when_words(now, created);
    assert!(!day.contains("ago") && m.starts_with(&format!("Created {day}  ")), "past a day, the day: {m}");
    let short = super::chrome::short_when(now, created);
    assert!(drawn(&mut d, &mut app, &short), "the card too: {short}");
    // Last edited, the same way: past a day, the day it was.
    app.on_plain(Msg::SetDates { to: 0 });
    app.store.notes[note].modified = created;
    let m = meta(&mut d, &mut app);
    assert!(m.starts_with(&format!("Edited {day}  ")), "{m}");
    assert!(drawn(&mut d, &mut app, &short), "and its card");
    app.on_plain(Msg::SetDates { to: 1 });
    // A file born at its last save: the history knows better.
    let other = app.store.notes.iter().position(|n| n.title == "Welcome to Carnet").unwrap();
    app.store.notes[other].created = now + Duration::from_secs(3600);
    app.dated.clear();
    app.on_plain(Msg::OpenNote { note: other as u64 });
    d.frame(&mut app);
    // (Its time is in whole seconds, rounded.)
    assert!(app.store.notes[other].created <= SystemTime::now() + Duration::from_secs(1), "its first version's");
    app.on_plain(Msg::SetDates { to: 0 });
    assert!(!app.settings.dates_created);
    remove_test_dir(dir);
}

/// The wheel is Carnet's own feel until Settings' Advanced card picks one
/// of kui's presets, which the window takes the next frame; it's kept in
/// the file, and a reset of the look leaves it.
#[test]
fn the_wheel_is_carnets_until_a_preset_is_picked() {
    let (store, dir) = temp_store("wheel");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 4000.0).framing();
    d_frame_twice(&mut app, &mut d);
    let carnet = kui_native::WheelFeel { line_px: 60.0, response: 0.5, kick: 0.2 };
    assert_eq!(d.core.wheel_feel(), carnet, "Carnet's own from the first frame");
    app.settings_open = true;
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    let brisk = d.key_named("Brisk").expect("the preset in Advanced");
    d.click_key(&mut app, brisk);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.settings.wheel, crate::settings::Wheel::Brisk);
    assert_eq!(d.core.wheel_feel(), kui_native::WheelFeel::BRISK, "taken at once");
    assert!(app.settings.to_text().contains("\nwheel = brisk\n"));
    app.settings.reset_look();
    assert_eq!(app.settings.wheel, crate::settings::Wheel::Brisk, "not a look");
    remove_test_dir(dir);
}

/// Reset puts the Appearance and Writing cards back as they first were,
/// asked first; the library and the notes folder stay.
#[test]
fn a_reset_puts_the_look_back_when_asked_twice() {
    let (store, dir) = temp_store("reset-look");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 3200.0).framing();
    app.settings_open = true;
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    let enabled = |d: &mut Drive, label: &str| d.core.nodes().iter().any(|n| n.label.as_deref() == Some(label) && !n.flags.contains(&"disabled"));
    assert!(!enabled(&mut d, "reset"), "nothing to reset yet");
    app.on_plain(Msg::SetSize { by: 3 });
    app.on_plain(Msg::SetPalette { to: 2 });
    app.on_plain(Msg::SetFocus { to: 1 });
    app.on_plain(Msg::SetSort { to: 1 });
    app.on_plain(Msg::SetWindow { to: 0 });
    d_frame_twice(&mut app, &mut d);
    let reset = d.key_of("reset").expect("the button");
    d.click_key(&mut app, reset);
    d_frame_twice(&mut app, &mut d);
    assert!(app.reset_armed && drawn(&mut d, &mut app, "Yes, reset") && drawn(&mut d, &mut app, "Cancel"));
    assert_eq!(app.settings.size, 19, "asked first: nothing yet");
    // Anything else done first: asked again.
    app.on_plain(Msg::SetTheme { to: 2 });
    assert!(!app.reset_armed);
    app.on_plain(Msg::ResetLook);
    app.on_plain(Msg::ResetLookCancel);
    assert!(!app.reset_armed && app.settings.size == 19);
    app.on_plain(Msg::ResetLook);
    d_frame_twice(&mut app, &mut d);
    let yes = d.key_of("reset-yes").unwrap();
    d.click_key(&mut app, yes);
    d_frame_twice(&mut app, &mut d);
    let s = &app.settings;
    assert_eq!((s.size, s.palette, s.focus, s.theme), (16, crate::design::Palette::Linen, false, ThemePref::System));
    assert_eq!(s.sort, Sort::Title, "the library's order stays");
    assert!(s.translucent, "the window back to translucent, as at first");
    assert!(drawn(&mut d, &mut app, "Colors, fonts, sizes, the window and the writing options, back as they were at first (the window from the next start). Notes, the library and sync stay as they are."));
    assert!(drawn(&mut d, &mut app, "The desktop through the library, blurred; from the next start"), "the Window row says when");
    assert!(!app.reset_armed && !enabled(&mut d, "reset"));
    assert!(app.notice.as_ref().is_some_and(|n| n.text == "Appearance and writing reset; the window from the next start"), "{:?}", app.notice.as_ref().map(|n| &n.text));
    // Solid alone: Reset puts it back too.
    app.on_plain(Msg::SetWindow { to: 0 });
    d_frame_twice(&mut app, &mut d);
    assert!(enabled(&mut d, "reset"), "the window is reset too");
    app.on_plain(Msg::ResetLook);
    app.on_plain(Msg::ResetLook);
    assert!(app.settings.translucent);
    let _ = std::fs::remove_dir_all(dir);
}

/// A note's history and the Deleted folder list their rows under a line
/// of words: each row keyed apart from it, every node once.
#[test]
fn the_history_and_deleted_lists_draw_each_node_once() {
    let (mut app, mut d, dir) = two_versions("lists-once");
    let check = |what: &str, app: &mut Carnet, d: &mut Drive| {
        d.core.set_inspect(true);
        d.frame(app);
        d.frame(app);
        let twice = shared_keys(d);
        assert!(twice.is_empty(), "{what}: {twice:#?}");
    };
    app.on_plain(Msg::ShowHistory);
    check("the history", &mut app, &mut d);
    app.on_plain(Msg::CloseHistory);
    let folder = app.store.create_folder("", "Work").unwrap();
    app.move_note(app.current, &folder);
    app.delete_note_at(app.current);
    app.trash_folder("Work");
    app.notice = None;
    app.on_plain(Msg::ShowDeleted);
    check("Deleted", &mut app, &mut d);
    assert_eq!(app.trash.len(), 2);
    remove_test_dir(dir);
}

/// Settings → Sync → Notes on the server, on a device alone in a new
/// account: the vault (where the notes go) is not "a note not on this
/// device" with edits this device lacks, nor offered a way to fix it;
/// a room the server has and this device hasn't is said to be one. What's
/// shown says when it was checked, and is checked again on its own once
/// the account's notes change, without "Check again". And after a save,
/// sync asks for a turn though nothing else would wake the app.
#[test]
fn the_inspector_knows_the_vault_and_keeps_up() {
    use carnet_server::server::{self, Config};
    use std::time::{Duration, Instant};
    let data = std::env::temp_dir().join(format!("carnet-app-inspect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let invite = server::new_invite(&mut carnet_server::Sharded::open(&data).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None }, listener, async {
        let _ = stopped.await;
    }));
    let until = |what: &str, app: &mut Carnet, d: &mut Drive, done: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !done(app) {
            assert!(t0.elapsed() < Duration::from_secs(15), "timed out: {what}");
            d.frame(app);
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let (mut a, mut da, dir_a) = fresh("inspect");
    a.sync_server = origin.clone();
    a.sync_code = invite.text();
    a.on_plain(Msg::SyncStart);
    until("online", &mut a, &mut da, &|a| a.sync.status == crate::sync::Status::Online);
    type_in(&mut a, &mut da, "Ferry times");
    a.save_current();
    da.frame(&mut a);
    assert!(a.sync.due().is_some(), "a turn asked for after a save");
    until("all sent and done", &mut a, &mut da, &|a| a.sync.due().is_none() && a.sync.is_idle());
    a.settings_open = true;
    a.on_plain(Msg::SyncInspector);
    until("inspected", &mut a, &mut da, &|a| {
        a.sync.inspection.as_ref().is_some_and(|(rooms, _)| rooms.len() >= 2 && rooms.iter().any(|r| r.room == carnet_sync::vault::ROOM))
    });
    let shows = |d: &Drive, s: &str| d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.contains(s)));
    let _ = drawn(&mut da, &mut a, "");
    assert!(!shows(&da, "not on this device"), "the vault taken for a note this device lacks");
    assert!(!shows(&da, "Ways to fix"), "nothing to fix");
    assert!(shows(&da, "notes are up to date"));
    assert!(shows(&da, "Checked just now"));
    assert_eq!(super::settings_view::sync_room_title(&a.store, carnet_sync::vault::ROOM), "Folders and note names");
    assert_eq!(super::settings_view::sync_room_title(&a.store, &carnet_sync::Key::random().to_string()), "A note not on this device yet");

    // A note saved: the list is out of date, says so, and is asked again
    // on its own; so is what the account keeps.
    until("what the account keeps", &mut a, &mut da, &|a| a.sync.usage.is_some());
    let kept = a.sync.usage.as_ref().unwrap().notes;
    let before = a.sync.inspection.as_ref().unwrap().0.len();
    a.new_note();
    da.frame(&mut a);
    type_in(&mut a, &mut da, "Packing");
    a.save_current();
    a.settings_open = true;
    until("changed since", &mut a, &mut da, &|a| a.sync.inspection_stale());
    let _ = drawn(&mut da, &mut a, "");
    assert!(shows(&da, "Changed since"));
    until("checked again, the new note in it", &mut a, &mut da, &|a| {
        !a.sync.inspection_stale() && a.sync.inspection.as_ref().is_some_and(|(rooms, _)| rooms.len() > before)
    });
    until("more kept", &mut a, &mut da, &|a| a.sync.usage.as_ref().is_some_and(|u| u.notes > kept));

    drop(a);
    let _ = stop.send(());
    let _ = rt.block_on(served);
    remove_test_dir(dir_a);
    let _ = std::fs::remove_dir_all(&data);
}

/// Five quick clicks on the Settings page's title turn the debug
/// information (the frame-time graph) on, and say so; five more, off.
/// Slow clicks don't count, and nothing of it is saved.
#[test]
fn five_quick_clicks_on_the_settings_title_turn_the_debug_information_on_and_off() {
    let (mut app, mut d, dir) = body("debug-clicks");
    app.on_settings(Msg::ShowSettings);
    d_frame_twice(&mut app, &mut d);
    let title = d.key_of("settings-title").expect("the page's title");
    for _ in 0..4 {
        d.click_key(&mut app, title);
    }
    assert!(!app.hud, "four are not enough");
    d.click_key(&mut app, title);
    assert!(app.hud, "the fifth turns it on");
    d_frame_twice(&mut app, &mut d);
    assert!(drawn(&mut d, &mut app, "Debug information on"));
    // With it on, F12 opens kui's devtools in any build.
    key(&mut app, &mut d, "f12");
    assert!(d.core.devtools());
    key(&mut app, &mut d, "f12");
    for _ in 0..5 {
        d.click_key(&mut app, title);
    }
    assert!(!app.hud);
    d_frame_twice(&mut app, &mut d);
    assert!(drawn(&mut d, &mut app, "Debug information off"));
    assert!(!app.settings.to_text().contains("debug"), "for this run only");

    // A pause longer than the run allows starts counting again.
    let t0 = Instant::now();
    for i in 0..5 {
        app.title_click(t0 + TITLE_CLICKS * 2 * i, &mut d.core);
    }
    assert!(!app.hud, "slow clicks");
    for i in 0..5 {
        app.title_click(t0 + TITLE_CLICKS * 20 + TITLE_CLICKS / 2 * i, &mut d.core);
    }
    assert!(app.hud, "quick ones");
    let _ = std::fs::remove_dir_all(dir);
}

/// A newer Carnet, as GitHub says: told once in a notice (and kept told
/// in the settings, so a later start doesn't say it again); Settings →
/// About says it while it's out, its Download opens the release's page.
/// Checking off is saved, and nothing is asked.
#[test]
fn a_newer_version_is_told_once_and_settings_offer_its_download() {
    use carnet_sync::http::Answer;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;
    type Answers = Rc<RefCell<Vec<mpsc::Sender<Result<Answer, String>>>>>;
    let github = |answers: &Answers| {
        let a = answers.clone();
        crate::update::Updates::with(Some(Box::new(move |_, _| {
            let (tx, rx) = mpsc::channel();
            a.borrow_mut().push(tx);
            Some(rx)
        })))
    };
    let release = include_str!("../assets/fixtures/github-release.json").replace("\"v0.1.2\"", "\"v999.0.0\"");
    let answer = || Ok(Answer { status: 200, body: release.as_bytes().to_vec(), ..Answer::default() });
    let told = "Carnet 999.0.0 is out: Settings has the download";

    // Settings open, the window tall enough to show them whole.
    let (mut app, mut d, dir) = tall("updates");
    let answers = Answers::default();
    app.updates = github(&answers);
    d.frame(&mut app);
    assert_eq!(answers.borrow().len(), 1, "asked at start");
    answers.borrow()[0].send(answer()).unwrap();
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.notice.as_ref().map(|n| n.text.as_str()), Some(told));
    assert_eq!(app.settings.told_update, "999.0.0");
    app.notice = None;
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_none(), "told once");
    assert_eq!(answers.borrow().len(), 1, "not asked again today");

    assert!(drawn(&mut d, &mut app, &format!("Carnet {}", env!("CARGO_PKG_VERSION"))));
    assert!(drawn(&mut d, &mut app, "Carnet 999.0.0 is out"));
    assert!(drawn(&mut d, &mut app, "Check for updates"));
    store::OPENED.with_borrow_mut(std::mem::take);
    let download = d.key_of("update-download").expect("Download");
    d.click_key(&mut app, download);
    assert_eq!(store::OPENED.with_borrow_mut(std::mem::take), ["https://github.com/qxuken/carnet/releases/tag/v999.0.0"]);

    // A later start, the same release found: not told again.
    let settings = app.settings.clone();
    let (store2, dir2) = temp_store("updates-later");
    let mut later = Carnet::with_settings(store2, settings);
    let mut d2 = Drive::new(Core::new(), 1100.0, 760.0).framing();
    let answers = Answers::default();
    later.updates = github(&answers);
    d2.frame(&mut later);
    answers.borrow()[0].send(answer()).unwrap();
    d_frame_twice(&mut later, &mut d2);
    assert!(later.notice.is_none(), "told before");
    assert!(later.updates.newer().is_some());

    // Off: saved so, and nothing asked.
    app.on_settings(Msg::SetCheckUpdates { to: 0 });
    assert!(app.settings.to_text().contains("check_updates = off\n"));
    let answers = Answers::default();
    app.updates = github(&answers);
    d_frame_twice(&mut app, &mut d);
    assert!(answers.borrow().is_empty());
    assert!(!drawn(&mut d, &mut app, "Carnet 999.0.0 is out"), "nothing known");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}

/// A selection over several lines: the toolbar's list tools and their
/// shortcuts make every one of them an item; again, every one back into
/// a paragraph. A mix is made all one kind first. Headings and quotes
/// the same way.
#[test]
fn list_tools_turn_every_selected_line_and_back() {
    let (mut app, mut d, dir) = body("turn-many");
    for (i, w) in ["milk", "eggs", "bread"].iter().enumerate() {
        if i > 0 {
            key(&mut app, &mut d, "enter");
        }
        type_in(&mut app, &mut d, w);
    }
    // From the end of the last line up into the first, by the keyboard.
    key_with(&mut app, &mut d, "up", SHIFT);
    key_with(&mut app, &mut d, "up", SHIFT);
    key_with(&mut app, &mut d, "home", SHIFT);
    d_frame_twice(&mut app, &mut d);
    let kinds = |app: &Carnet| -> Vec<&'static str> {
        blocks(app)[..3]
            .iter()
            .map(|b| match &b.kind {
                Kind::Item { style: ListStyle::Bullet, .. } => "bullet",
                Kind::Item { style: ListStyle::Number, .. } => "number",
                Kind::Item { style: ListStyle::Todo, .. } => "todo",
                Kind::Heading { .. } => "heading",
                Kind::Quote(_) => "quote",
                Kind::Text(_) => "text",
                _ => "other",
            })
            .collect()
    };
    let tool = d.key_of("t-bullet").expect("the toolbar's Bulleted list");
    d.click_key(&mut app, tool);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(kinds(&app), ["bullet"; 3], "every line, not the caret's");
    assert_eq!(page::touched(app.doc(), app.caret), 0..=2, "still all selected");
    assert!(page::turned_to(app.doc(), app.caret, TURN_BULLET), "the tool shows on");
    let tool = d.key_of("t-bullet").unwrap();
    d.click_key(&mut app, tool);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(kinds(&app), ["text"; 3], "all back to paragraphs");
    assert_eq!(texts(&app)[..3], ["milk", "eggs", "bread"]);

    // The shortcuts: ⌥⌘4 a checklist of all three, and back.
    key_with(&mut app, &mut d, "4", CTRL_ALT);
    assert_eq!(kinds(&app), ["todo"; 3]);
    key_with(&mut app, &mut d, "4", CTRL_ALT);
    assert_eq!(kinds(&app), ["text"; 3]);
    key_with(&mut app, &mut d, "6", CTRL_ALT);
    assert_eq!(kinds(&app), ["number"; 3]);
    key_with(&mut app, &mut d, "6", CTRL_ALT);
    assert_eq!(kinds(&app), ["text"; 3]);

    // A mix: one a bullet, the others not — all made bullets first.
    app.caret = page::select_blocks(app.doc(), 1, 1);
    key_with(&mut app, &mut d, "5", CTRL_ALT);
    assert_eq!(kinds(&app), ["text", "bullet", "text"]);
    app.caret = page::select_blocks(app.doc(), 0, 2);
    key_with(&mut app, &mut d, "5", CTRL_ALT);
    assert_eq!(kinds(&app), ["bullet"; 3]);
    // Bullets made a checklist: all of them, and not back to text.
    key_with(&mut app, &mut d, "4", CTRL_ALT);
    assert_eq!(kinds(&app), ["todo"; 3]);
    key_with(&mut app, &mut d, "4", CTRL_ALT);
    assert_eq!(kinds(&app), ["text"; 3]);

    // Headings and quotes share the way.
    key_with(&mut app, &mut d, "2", CTRL_ALT);
    assert_eq!(kinds(&app), ["heading"; 3]);
    key_with(&mut app, &mut d, "2", CTRL_ALT);
    assert_eq!(kinds(&app), ["text"; 3]);
    key_with(&mut app, &mut d, "7", CTRL_ALT);
    assert_eq!(kinds(&app), ["quote"; 3]);
    key_with(&mut app, &mut d, "7", CTRL_ALT);
    assert_eq!(kinds(&app), ["text"; 3]);
    // Text on text stays text.
    key_with(&mut app, &mut d, "0", CTRL_ALT);
    assert_eq!(kinds(&app), ["text"; 3]);
    // Undone in one step.
    key_with(&mut app, &mut d, "5", CTRL_ALT);
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(kinds(&app), ["text"; 3]);
    let _ = std::fs::remove_dir_all(dir);
}

/// A note from Apple Notes' export, its lines ended by line breaks: a
/// click on its first line and the toolbar's Bulleted list make that line
/// an item, and the rest of the file stays as it was, byte for byte; the
/// shortcut takes it back to a paragraph of its own, and Ctrl+Z to the
/// file as it came. ⌥⌘6 numbers a line in the middle of a paragraph.
#[test]
fn an_exported_notes_line_is_turned_alone() {
    const EXPORT: &str = include_str!("../crates/carnet-core/tests/fixtures/apple-notes.md");
    let (store, dir) = temp_store("export-lines");
    drop(store);
    std::fs::write(dir.join("поездки.md"), EXPORT).unwrap();
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    d.frame(&mut app);
    let i = app.store.notes.iter().position(|n| n.title == "поездки").unwrap();
    app.open(i);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let written = |app: &mut Carnet| {
        app.save_current();
        std::fs::read_to_string(dir.join("поездки.md")).unwrap()
    };
    assert_eq!(written(&mut app), EXPORT);
    // On the page (not the note's card in the library): the paragraph.
    let first = d.core.nodes().iter().find(|n| n.text.as_deref() == Some("путь: жильё\nhostel / ryokan")).expect("the first paragraph").rect;
    d.click(&mut app, first.x + 30.0, first.y + 8.0);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(page::touched(app.doc(), app.caret), 0..=0);
    let tool = d.key_of("t-bullet").expect("the toolbar's Bulleted list");
    d.click_key(&mut app, tool);
    d_frame_twice(&mut app, &mut d);
    let one = EXPORT.replacen("**путь:** жильё   \nhostel / ryokan  \n", "- **путь:** жильё\n\nhostel / ryokan  \n", 1);
    assert_eq!(written(&mut app), one);
    assert!(page::turned_to(app.doc(), app.caret, TURN_BULLET), "the tool shows on");
    key_with(&mut app, &mut d, "5", CTRL_ALT);
    let back = EXPORT.replacen("**путь:** жильё   \nhostel / ryokan  \n", "**путь:** жильё\n\nhostel / ryokan  \n", 1);
    assert_eq!(written(&mut app), back);
    key_with(&mut app, &mut d, "z", CTRL);
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(written(&mut app), EXPORT, "undone, the file as it came");

    // A line in the middle of the longest paragraph, by the keyboard.
    let big = app.doc().blocks().iter().position(|b| b.text_ref().is_some_and(|t| t.starts_with("**дом:** мысли"))).unwrap();
    let slot = Slot::block(app.doc().blocks()[big].id);
    let at = page::text(app.doc(), slot).find("picture").unwrap();
    app.caret = Caret::at(Pos { slot, byte: at + 3 });
    key_with(&mut app, &mut d, "6", CTRL_ALT);
    assert_eq!(
        written(&mut app),
        EXPORT.replacen("коридоре  \npicture rail  \nskylight", "коридоре  \n\n1. picture rail\n\nskylight", 1)
    );
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A line broken as Markdown's hard break (two spaces, a backslash): the
/// page draws no mark of it; End puts the caret after the line's last
/// letter, what is typed there goes before the marks, → crosses the break
/// in one step, and Backspace there takes it, marks and all. Untouched,
/// the file is written back as it was.
#[test]
fn a_hard_line_break_is_one_break_on_the_page() {
    for (n, mark) in ["  ", "\\"].into_iter().enumerate() {
        let md = format!("# Breaks\n\nstay{mark}\napart\n");
        let (store, dir) = temp_store(&format!("hard-break-{n}"));
        drop(store);
        std::fs::write(dir.join("Breaks.md"), &md).unwrap();
        let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
        let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
        d.core.set_inspect(true);
        d.frame(&mut app);
        let i = app.store.notes.iter().position(|n| n.title == "Breaks").unwrap();
        app.open(i);
        for _ in 0..4 {
            d.frame(&mut app);
        }
        let written = |app: &mut Carnet| {
            app.save_current();
            std::fs::read_to_string(dir.join("Breaks.md")).unwrap()
        };
        assert_eq!(written(&mut app), md);
        let para = d.core.nodes().iter().find(|n| n.text.as_deref() == Some("stay\napart")).expect("the paragraph, its marks hidden").rect;
        d.click(&mut app, para.x + 10.0, para.y + 6.0);
        d_frame_twice(&mut app, &mut d);
        key(&mut app, &mut d, "end");
        let slot = app.caret.head.slot;
        assert_eq!(app.caret.head.byte, 4, "End: after the line's last letter");
        type_in(&mut app, &mut d, "s");
        assert_eq!(page::text(app.doc(), slot), format!("stays{mark}\napart"));
        key(&mut app, &mut d, "right");
        assert_eq!(app.caret.head.byte, 5 + mark.len() + 1, "one step over the break");
        key(&mut app, &mut d, "backspace");
        assert_eq!(page::text(app.doc(), slot), "staysapart");
        assert_eq!(written(&mut app), "# Breaks\n\nstaysapart\n");
        let w = d.warnings();
        assert!(w.is_empty(), "{w:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The finding: Shift+Enter made no new line until a letter was typed on
/// it, the caret staying at the end of the line above. The line is there
/// at once, the caret at its start; `/` on it offers the blocks, and the
/// one chosen takes that line alone. Saved before anything is typed on
/// it, the break reads back as it was.
#[test]
fn shift_enter_opens_its_line_at_once() {
    let (mut app, mut d, dir) = body("shift-enter");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "hello");
    let slot = app.caret.head.slot;
    let first = app.rect_at(&d.core, slot, 0).unwrap();
    key_with(&mut app, &mut d, "enter", SHIFT);
    d_frame_twice(&mut app, &mut d);
    assert_eq!((page::text(app.doc(), slot), app.caret.head.byte), ("hello\n", 6));
    let r = app.rect_at(&d.core, slot, 6).unwrap();
    assert!(r.y >= first.y + first.h - 0.5 && (r.x - first.x).abs() < 0.5, "on a line of its own, at its start: {r:?} under {first:?}");
    app.save_current();
    let path = app.store.notes[app.current].path.clone().unwrap();
    let md = std::fs::read_to_string(&path).unwrap();
    let (_, back) = carnet_core::doc::from_markdown(&md);
    assert_eq!(back.blocks()[0].text_ref(), Some("hello\n"), "{md:?}");

    type_in(&mut app, &mut d, "/");
    d.frame(&mut app);
    assert!(app.slash_now().is_some(), "{:?}", texts(&app));
    assert!(d.core.nodes().iter().any(|n| n.text.as_deref() == Some("Heading 1")), "the menu shows");
    type_in(&mut app, &mut d, "h2");
    key(&mut app, &mut d, "enter");
    assert_eq!(texts(&app)[0], "hello");
    assert!(matches!(&blocks(&app)[1].kind, Kind::Heading { level: 2, text } if text.is_empty()), "{:?}", texts(&app));
    // Mid-sentence it is a slash.
    type_in(&mut app, &mut d, "either/or");
    d.frame(&mut app);
    assert!(app.slash_now().is_none());
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding, in a note of the user's: a line ending highlighted and in
/// a hard break's two spaces, `Open <mark …>terminal</mark>  `, and `/` on
/// a new line under it offered nothing. Enter took the spaces down to the
/// new paragraph, the caret before them; Shift+Enter broke the line
/// inside the highlight. Either way the new line now starts clean, the
/// menu shows on it, and the file keeps the line above as it was.
#[test]
fn a_slash_on_the_line_after_a_highlight_offers_the_blocks() {
    let line = "Open <mark style=\"background: #E2D5F0CC\">terminal</mark>  ";
    let rest = "- Navigate to vpn folder   \n\n```\n$ z vpn\n```\n\n- To start connection  \n";
    let md = format!("# vpn\n\n{line}\n\n{rest}");
    for (n, mods) in [KeyMods::NONE, SHIFT].into_iter().enumerate() {
        let (store, dir) = temp_store(&format!("slash-after-mark-{n}"));
        drop(store);
        std::fs::write(dir.join("vpn.md"), &md).unwrap();
        let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
        let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
        d.core.set_inspect(true);
        d.frame(&mut app);
        let i = app.store.notes.iter().position(|n| n.title == "vpn").unwrap();
        app.open(i);
        for _ in 0..4 {
            d.frame(&mut app);
        }
        let written = |app: &mut Carnet| {
            app.save_current();
            std::fs::read_to_string(dir.join("vpn.md")).unwrap()
        };
        // On the page, not the note's line in the list.
        let keys: Vec<Key> = app.slot_keys.values().copied().collect();
        let para = d.core.nodes().iter().find(|n| n.text.as_deref() == Some("Open terminal") && keys.contains(&n.key)).expect("the line, its marks hidden").rect;
        d.click(&mut app, para.x + 10.0, para.y + 6.0);
        d_frame_twice(&mut app, &mut d);
        key(&mut app, &mut d, "end");
        let above = app.caret.head.slot;
        let end = app.rect_at(&d.core, above, app.caret.head.byte).unwrap();
        key_with(&mut app, &mut d, "enter", mods);
        d_frame_twice(&mut app, &mut d);
        if mods.shift {
            assert_eq!(page::text(app.doc(), above), format!("{line}\n"), "the break after the spaces");
        } else {
            assert_eq!(texts(&app)[..3], [line, "", "[ ]Navigate to vpn folder   "], "the spaces stay above");
        }
        // An empty paragraph's caret is its line's start, unmeasured.
        let k = app.slot_keys.get(&app.caret.head.slot).copied().unwrap();
        let line_of = d.core.nodes().iter().find(|n| n.key == k).map(|n| n.rect);
        let r = app.rect_at(&d.core, app.caret.head.slot, app.caret.head.byte).or(line_of).unwrap();
        assert!(r.y >= end.y + end.h - 0.5, "the caret on the new line: {r:?} under {end:?}");
        let file = written(&mut app);
        if mods.shift {
            // The break with nothing after it yet: a line of a backslash,
            // read back as the empty line it is.
            assert_eq!(file, format!("# vpn\n\n{line}\n\\\n\n{rest}"));
            let (_, back) = carnet_core::doc::from_markdown(&file);
            assert_eq!(back.blocks()[0].text_ref(), Some(format!("{line}\n").as_str()));
        } else {
            assert!(file.starts_with(&format!("# vpn\n\n{line}\n\n")), "{file:?}");
        }
        type_in(&mut app, &mut d, "/");
        d.frame(&mut app);
        assert!(app.slash_now().is_some(), "{mods:?}: {:?}", texts(&app));
        assert!(d.core.nodes().iter().any(|n| n.text.as_deref() == Some("Table")), "the menu shows");
        type_in(&mut app, &mut d, "code");
        key(&mut app, &mut d, "enter");
        assert_eq!(texts(&app)[0], line, "{mods:?}: the line above as it was");
        assert!(matches!(&blocks(&app)[1].kind, Kind::Raw(r) if r.starts_with("```")), "{:?}", texts(&app));
        let w = d.warnings();
        assert!(w.is_empty(), "{w:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A picture that's the user's own file, beside the note: drawn faint with
/// a mark, and selected, a bar that says so. Moved into Carnet, it's in
/// the attachments folder and the note shows it there; the notice's Undo
/// (Ctrl+Z) puts the file back, and an undo of the typing before never
/// brings back a path that leads nowhere. Copied, only this note shows
/// the copy, and Ctrl+Z takes the edit back.
#[test]
fn a_file_that_isnt_a_picture_is_never_taken_into_carnet() {
    let (mut app, mut d, dir) = body("theirs-not-a-picture");
    type_in(&mut app, &mut d, "Above");
    // A key, named as a picture would be or not at all.
    let key = dir.join("id_rsa");
    std::fs::write(&key, "-----BEGIN OPENSSH PRIVATE KEY-----\n").unwrap();
    let fake = dir.join("fake.png");
    std::fs::write(&fake, b"\x89PNG\r\n\x1a\n not a picture past its first bytes").unwrap();
    for src in ["id_rsa", "fake.png"] {
        let image = Block::new(Kind::Image { src: src.into(), alt: String::new(), float: Float::None, width: None, wiki: false });
        app.edit(Step::Other, |doc, _| {
            doc.push(image);
            true
        });
    }
    app.save_current();
    for src in ["id_rsa", "fake.png"] {
        let i = blocks(&app).iter().position(|b| matches!(&b.kind, Kind::Image { src: s, .. } if s == src)).unwrap();
        app.caret = page::select_blocks(app.doc(), i, i);
        for _ in 0..4 {
            d.frame(&mut app);
        }
        assert!(d.key_of("move-in").is_none() && d.key_of("copy-in").is_none(), "{src}: no way in offered");
        // Asked all the same (a menu from before): nothing taken.
        app.take_in_picture(false);
        app.take_in_picture(true);
    }
    assert!(key.is_file() && fake.is_file());
    assert!(!dir.join("attachments").exists(), "nothing in Carnet's folder");
    assert_eq!(image_blocks(&app), ["id_rsa", "fake.png"]);
}

#[test]
fn the_users_own_picture_is_marked_and_taken_into_carnet() {
    let (mut app, mut d, dir) = body("theirs");
    type_in(&mut app, &mut d, "Above");
    let hill = png_file(&dir, "hill.png");
    let n = app.current;
    let image = Block::new(Kind::Image { src: "hill.png".into(), alt: String::new(), float: Float::None, width: None, wiki: false });
    app.edit(Step::Other, |doc, _| {
        doc.push(image);
        true
    });
    app.save_current();
    let i = blocks(&app).iter().position(|b| matches!(b.kind, Kind::Image { .. })).unwrap();
    app.caret = page::select_blocks(app.doc(), i, i);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    assert!(d.key_of("theirs").is_some(), "marked");
    let into = d.key_of("move-in").expect("the bar's Move into Carnet");
    assert!(d.key_of("copy-in").is_some());
    d.click_key(&mut app, into);
    d.frame(&mut app);
    let moved = dir.join("attachments/hill.png");
    assert!(moved.is_file() && !hill.exists(), "{:?} {:?}", app.error, app.notice.as_ref().map(|n| &n.text));
    assert_eq!(image_blocks(&app), ["attachments/hill.png"]);
    let md = std::fs::read_to_string(app.store.notes[n].path.clone().unwrap()).unwrap();
    assert!(md.contains("](attachments/hill.png)"), "{md}");
    for _ in 0..2 {
        d.frame(&mut app);
    }
    assert!(d.key_of("theirs").is_none(), "Carnet's now");
    // The notice's Undo: back where it was.
    let undo = d.key_of("undo").expect("the notice's Undo");
    d.click_key(&mut app, undo);
    d.frame(&mut app);
    assert!(hill.is_file() && !moved.exists(), "{:?}", app.error);
    assert_eq!(image_blocks(&app), ["hill.png"]);
    // An undo of the edit before (the picture put in) takes it out, as it
    // would have; one more and nothing is left to undo.
    key_with(&mut app, &mut d, "z", CTRL);
    assert!(image_blocks(&app).is_empty());
    key_with(&mut app, &mut d, "y", CTRL);
    assert_eq!(image_blocks(&app), ["hill.png"], "redone as it is now");

    // Copied: this note shows the copy; the file stays.
    let i = blocks(&app).iter().position(|b| matches!(b.kind, Kind::Image { .. })).unwrap();
    app.caret = page::select_blocks(app.doc(), i, i);
    d.frame(&mut app);
    d.frame(&mut app);
    let copy = d.key_of("copy-in").expect("the bar's Copy into Carnet");
    d.click_key(&mut app, copy);
    d.frame(&mut app);
    assert!(hill.is_file() && moved.is_file());
    assert_eq!(image_blocks(&app), ["attachments/hill.png"]);
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(image_blocks(&app), ["hill.png"], "the edit taken back");
    let _ = std::fs::remove_dir_all(dir);
}

/// A note showing the user's own picture `hill.png`, the picture selected,
/// copied into Carnet from its bar.
fn copied_in(name: &str) -> (Carnet, Drive, std::path::PathBuf) {
    let (mut app, mut d, dir) = body(name);
    type_in(&mut app, &mut d, "Above");
    png_file(&dir, "hill.png");
    let image = Block::new(Kind::Image { src: "hill.png".into(), alt: String::new(), float: Float::None, width: None, wiki: false });
    app.edit(Step::Other, |doc, _| {
        doc.push(image);
        true
    });
    app.save_current();
    let i = blocks(&app).iter().position(|b| matches!(b.kind, Kind::Image { .. })).unwrap();
    app.caret = page::select_blocks(app.doc(), i, i);
    d.core.set_inspect(true);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let copy = d.key_of("copy-in").expect("the bar's Copy into Carnet");
    d.click_key(&mut app, copy);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(image_blocks(&app), ["attachments/hill.png"]);
    assert!(dir.join("attachments/hill.png").is_file() && dir.join("hill.png").is_file());
    (app, d, dir)
}

/// The notice's Undo of a copy into Carnet: the note shows the user's
/// file again, and the copy, which nothing else shows, goes — to the
/// trash, where the sweep would have put it — even when another app
/// changed another note while the window was in the background (which
/// once took the open note's undo with it, and the Undo did nothing).
#[test]
fn a_copy_into_carnet_undone_takes_the_copy_away() {
    let (mut app, mut d, dir) = copied_in("copy-undone");
    d.core.set_focused(false);
    d.frame(&mut app);
    std::fs::write(dir.join("Elsewhere.md"), "# Elsewhere\n\nWritten by another app.\n").unwrap();
    d.core.set_focused(true);
    d_frame_twice(&mut app, &mut d);
    assert!(app.store.notes.iter().any(|n| n.title == "Elsewhere"), "the other app's note read");
    let undo = keyed_rect(&mut d, "undo");
    d.click(&mut app, undo.x + undo.w / 2.0, undo.y + undo.h / 2.0);
    d.frame(&mut app);
    assert_eq!(image_blocks(&app), ["hill.png"], "the user's file again");
    assert!(dir.join("hill.png").is_file(), "the user's file stays");
    assert!(!dir.join("attachments/hill.png").exists(), "the copy goes");
    assert!(dir.join(".trash/attachments/hill.png").is_file(), "to the trash");
    let md = std::fs::read_to_string(app.store.notes[app.current].path.clone().unwrap()).unwrap();
    assert!(md.contains("](hill.png)"), "{md}");
    // A redo brings back no path to the copy, gone.
    key_with(&mut app, &mut d, "y", CTRL);
    assert_eq!(image_blocks(&app), ["hill.png"]);

    // ⌘Z while the notice shows is its Undo: the copy goes too.
    let i = blocks(&app).iter().position(|b| matches!(b.kind, Kind::Image { .. })).unwrap();
    app.caret = page::select_blocks(app.doc(), i, i);
    d_frame_twice(&mut app, &mut d);
    let copy = d.key_of("copy-in").expect("the bar's Copy into Carnet");
    d.click_key(&mut app, copy);
    d.frame(&mut app);
    assert_eq!(image_blocks(&app), ["attachments/hill.png"]);
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(image_blocks(&app), ["hill.png"]);
    assert!(!dir.join("attachments/hill.png").exists() && dir.join("hill.png").is_file());

    // Another note showing the copy keeps it: the Undo takes back the
    // edit alone.
    let again = d.key_of("copy-in").expect("the bar again");
    d.click_key(&mut app, again);
    d.frame(&mut app);
    let other = app.store.notes.iter().position(|n| n.title == "Elsewhere").unwrap();
    std::fs::write(app.store.notes[other].path.clone().unwrap(), "# Elsewhere\n\n![](attachments/hill.png)\n").unwrap();
    app.store.rescan();
    app.on_plain(Msg::Undo);
    d.frame(&mut app);
    assert_eq!(image_blocks(&app), ["hill.png"]);
    assert!(dir.join("attachments/hill.png").is_file(), "shown elsewhere, kept");
    let _ = std::fs::remove_dir_all(dir.join(".trash"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A notice stays while the pointer is on it, on its way to its Undo,
/// and while the window is in the background; else it goes after its
/// while.
#[test]
fn a_notice_stays_while_pointed_at_or_unseen() {
    let (mut app, mut d, dir) = copied_in("notice-stays");
    let long_ago = || Instant::now() - Duration::from_secs(60);
    let undo = keyed_rect(&mut d, "undo");
    d.move_to(&mut app, undo.x + undo.w / 2.0, undo.y + undo.h / 2.0);
    d.frame(&mut app);
    app.notice.as_mut().unwrap().at = long_ago();
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_some(), "pointed at its Undo");
    let words = keyed_rect(&mut d, "notice");
    d.move_to(&mut app, words.x + 10.0, words.y + words.h / 2.0);
    d.frame(&mut app);
    app.notice.as_mut().unwrap().at = long_ago();
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_some(), "pointed at its words");
    // The click lands: its Undo, at once.
    d.click(&mut app, undo.x + undo.w / 2.0, undo.y + undo.h / 2.0);
    d.frame(&mut app);
    assert_eq!(image_blocks(&app), ["hill.png"]);
    // Unseen, in the background: it waits.
    app.notify("Something happened", Some(Undo::Edit));
    d.move_to(&mut app, 700.0, 600.0);
    d.core.set_focused(false);
    d.frame(&mut app);
    app.notice.as_mut().unwrap().at = long_ago();
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_some(), "the window in the background");
    d.core.set_focused(true);
    d.frame(&mut app);
    app.notice.as_mut().unwrap().at = long_ago();
    d_frame_twice(&mut app, &mut d);
    assert!(app.notice.is_none(), "seen, and not pointed at: gone after its while");
    let _ = std::fs::remove_dir_all(dir.join(".trash"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A divider put in at the note's end leaves a line to write on; a
/// Backspace there selects the divider, drawn in the selection's wash and
/// the accent, and only the next one takes it.
#[test]
fn a_divider_is_selected_before_backspace_takes_it() {
    let (mut app, mut d, dir) = body("rule-select");
    d.core.set_inspect(true);
    type_in(&mut app, &mut d, "Words");
    app.on_plain(Msg::InsertDivider);
    d_frame_twice(&mut app, &mut d);
    assert_eq!(texts(&app), ["Words", "---", ""]);
    // The divider's block as drawn: the most washed of what is in its
    // row, the ornament's ink aside.
    let wash = |app: &Carnet, d: &Drive| {
        let id = blocks(app)[1].id;
        let r = app.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r).expect("the divider's row");
        let inside = |n: &Rect| n.x >= r.x && n.y >= r.y && n.x + n.w <= r.x + r.w && n.y + n.h <= r.y + r.h && n.w > r.w * 0.5;
        d.core.nodes().iter().filter(|n| inside(&n.rect)).map(|n| n.bg.a).fold(0.0, f32::max)
    };
    assert_eq!(wash(&app, &d), 0.0, "not selected: no wash");
    key(&mut app, &mut d, "backspace");
    d.frame(&mut app);
    assert_eq!(texts(&app), ["Words", "---", ""], "the last line stays");
    assert!(page::coverage(app.doc(), app.caret).whole.contains(&blocks(&app)[1].id), "the divider selected");
    assert!(wash(&app, &d) > 0.0, "and drawn so");
    key(&mut app, &mut d, "backspace");
    assert_eq!(texts(&app), ["Words", ""]);
    let _ = std::fs::remove_dir_all(dir);
}

/// The library's edge isn't drawn at rest; under the pointer it is one
/// solid line the window's height, in either kind of window, light or
/// dark: nothing of what is behind the window shows through it, and
/// nothing is laid over it.
#[test]
fn the_library_edge_is_solid_its_whole_height() {
    let (mut app, mut d, dir) = fresh("library-edge-solid");
    d.core.set_inspect(true);
    let edge = |d: &mut Drive| {
        let k = d.key_of("library-edge").expect("the library's edge");
        d.core.nodes().into_iter().find(|n| n.key == k).unwrap()
    };
    for (translucent, backdrop) in [(false, kui_native::Backdrop::Opaque), (true, kui_native::Backdrop::Blur), (true, kui_native::Backdrop::Transparent)] {
        for theme in [ThemePref::Light, ThemePref::Dark] {
            app.settings.translucent = translucent;
            app.settings.theme = theme;
            d.core.env.window.backdrop = backdrop;
            d.move_to(&mut app, 700.0, 300.0);
            d_frame_twice(&mut app, &mut d);
            let n = edge(&mut d);
            let what = format!("{backdrop:?} {theme:?}");
            assert_eq!(n.bg.a, 0.0, "not drawn at rest: {what}");
            assert_eq!((n.rect.y, n.rect.h), (0.0, d.viewport.h), "the window's height: {what}");
            // Nothing drawn over it: the fade under the toolbar, laid
            // across the page's first points, notched it.
            let over: Vec<_> = d
                .core
                .nodes()
                .into_iter()
                .filter(|o| o.key != n.key && o.float && o.layer >= n.layer && o.rect.w > 0.0 && o.rect.h > 0.0)
                .filter(|o| o.rect.x < n.rect.x + n.rect.w && n.rect.x < o.rect.x + o.rect.w)
                .filter(|o| o.rect.y < n.rect.y + n.rect.h && n.rect.y < o.rect.y + o.rect.h)
                .map(|o| (o.label, o.rect))
                .collect();
            assert!(over.is_empty(), "nothing over it: {what}: {over:?}");
            d.move_to(&mut app, n.rect.x + 3.0, 300.0);
            d_frame_twice(&mut app, &mut d);
            let hovered = edge(&mut d);
            assert_eq!(hovered.bg.a, 1.0, "solid under the pointer: {what}");
            assert_eq!((hovered.rect.y, hovered.rect.h), (0.0, d.viewport.h), "the window's height: {what}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Settings → Sync's waiting line, by the frame clock: a wait a fast
/// server ends within a frame or two says nothing; one that lasts says
/// "Waiting…" after a moment, then "All sent" for a moment more, and
/// asks for the frames that change it.
#[test]
fn the_waiting_line_waits_a_moment_and_says_all_sent() {
    use super::settings_view::{WaitLine, Waited, SENT_SHOWN_FOR, WAIT_SHOWN_AFTER};
    // A frame's wait: nothing at all, and nothing more to draw after.
    let mut w = WaitLine::default();
    assert_eq!(w.said(10.0, true), (None, Some(10.0 + WAIT_SHOWN_AFTER)));
    assert_eq!(w.said(10.016, false), (None, None));
    assert_eq!(w.said(10.0 + WAIT_SHOWN_AFTER, false), (None, None));
    // A real wait: "Waiting…" once it has lasted, while it lasts.
    let mut w = WaitLine::default();
    assert_eq!(w.said(20.0, true).0, None);
    assert_eq!(w.said(20.0 + WAIT_SHOWN_AFTER, true), (Some(Waited::Waiting), None));
    assert_eq!(w.said(25.0, true).0, Some(Waited::Waiting));
    // Sent: "All sent", and a frame asked for when it is to go.
    assert_eq!(w.said(26.0, false), (Some(Waited::Sent), Some(26.0 + SENT_SHOWN_FOR)));
    assert_eq!(w.said(27.0, false).0, Some(Waited::Sent));
    assert_eq!(w.said(26.0 + SENT_SHOWN_FOR, false), (None, None));
    // A new short wait while "All sent" shows: it stays, unbroken.
    let mut w = WaitLine::default();
    w.said(0.0, true);
    w.said(1.0, true);
    assert_eq!(w.said(2.0, false).0, Some(Waited::Sent));
    assert_eq!(w.said(2.5, true).0, Some(Waited::Sent));
    assert_eq!(w.said(2.6, false).0, Some(Waited::Sent));
    assert_eq!(w.said(2.0 + SENT_SHOWN_FOR, false).0, None);
}

/// A picture among a table cell's words: drawn in the cell, as wide as
/// the cell's room (it is wider, whatever width the note writes), its
/// shape kept, the words under it; one letter to the caret, chosen by a
/// press and taken whole by Backspace; one pasted with the caret in a cell
/// goes into the cell.
#[test]
fn a_picture_in_a_cell_is_drawn_in_it_and_taken_whole() {
    let (mut app, mut d, dir) = body("cell-picture");
    let rgba: Vec<u8> = (0..800 * 400).flat_map(|_| [30u8, 90, 160, 255]).collect();
    let png = crate::images::encode_png(800, 400, &rgba).unwrap();
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    std::fs::write(dir.join("attachments/wide.png"), png).unwrap();
    app.images.preload(&dir.join("attachments/wide.png"), crate::images::Decoded { w: 800, h: 400, rgba });
    let pic = "![|left|9000](attachments/wide.png)";
    let table = Table::from_cells(vec![vec!["img1".into(), "img2".into()], vec![format!("{pic}Words"), String::new()]]);
    app.store.notes[app.current].doc = Doc::new(vec![Block::new(Kind::Table(table)), Block::text("")]);
    app.caret = Caret::at(page::first(app.doc()));
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let Kind::Table(t) = &blocks(&app)[0].kind else { panic!() };
    let slot = Slot { block: blocks(&app)[0].id, cell: Some((t.rows[1].id, t.cols[0])) };
    let pieces = app.cell_pieces.get(&slot).expect("a cell of pieces").clone();
    assert_eq!(pieces.iter().map(|p| p.picture).collect::<Vec<_>>(), [true, false], "the picture, then its words");
    let r = d.core.layout_of(pieces[0].key).expect("the picture's box");
    let cell = *app.cell_rects.get(&slot).unwrap();
    assert!((r.w - (cell.w - 28.0)).abs() <= 2.0, "as wide as the cell's room: {r:?} in {cell:?}");
    assert!((r.w - 2.0 * r.h).abs() <= 1.0, "its shape: {r:?}");
    let image = d.core.nodes().iter().any(|n| n.role == Some(Role::Image) && (n.rect.w - r.w).abs() < 0.5);
    assert!(image, "an image node of the cell's width");
    // Its words below it.
    let words = app.rect_at(&d.core, slot, pic.len() + 2).unwrap();
    assert!(words.y > r.y + r.h - 1.0, "{words:?} under {r:?}");
    // A press on it chooses it whole; Backspace takes it whole.
    d.click(&mut app, r.x + r.w / 2.0, r.y + r.h / 2.0);
    d.frame(&mut app);
    assert_eq!((app.caret.anchor, app.caret.head), (Pos { slot, byte: 0 }, Pos { slot, byte: pic.len() }));
    key(&mut app, &mut d, "backspace");
    assert_eq!(page::text(app.doc(), slot), "Words");
    // → chooses a picture whole, and → again is past it.
    if let Some(Kind::Table(t)) = app.store.notes[app.current].doc.get_mut(slot.block).map(|b| &mut b.kind) {
        t.rows[1].cells[0] = format!("a{pic}b");
    }
    app.caret = Caret::at(Pos { slot, byte: 1 });
    d.frame(&mut app);
    key(&mut app, &mut d, "right");
    assert_eq!((app.caret.anchor, app.caret.head), (Pos { slot, byte: 1 }, Pos { slot, byte: 1 + pic.len() }));
    key(&mut app, &mut d, "right");
    assert_eq!(app.caret, Caret::at(Pos { slot, byte: 1 + pic.len() }));
    // Pasted with the caret in a cell: into the cell, after the caret.
    let small: Vec<u8> = (0..12).flat_map(|_| [10u8, 20, 30, 255]).collect();
    app.paste_picture(4, 3, &small);
    d.frame(&mut app);
    let now = page::text(app.doc(), slot).to_string();
    assert!(now.starts_with(&format!("a{pic}![](attachments/pasted-")) && now.ends_with(".png)b"), "{now}");
    assert_eq!(blocks(&app).len(), 2, "no block of its own");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A picture on its own line carried onto a table's cell goes among its
/// words, its line gone, written plainly (no width nor side: the cell
/// decides), the cell ringed while it is over it and no drop line; one in
/// a cell carried out between lines is a line of its own again, its own
/// width. Each is one step of the history.
#[test]
fn a_picture_is_carried_into_a_cell_and_out_again() {
    let (mut app, mut d, dir) = body("cell-dnd");
    let rgba: Vec<u8> = (0..200 * 100).flat_map(|_| [30u8, 90, 160, 255]).collect();
    let png = crate::images::encode_png(200, 100, &rgba).unwrap();
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    std::fs::write(dir.join("attachments/wide.png"), png).unwrap();
    app.images.preload(&dir.join("attachments/wide.png"), crate::images::Decoded { w: 200, h: 100, rgba });
    let pic = Block::new(Kind::Image { src: "attachments/wide.png".into(), alt: String::new(), width: Some(150), float: Float::Right, wiki: false });
    let id = pic.id;
    let table = Table::from_cells(vec![vec!["A".into(), "B".into()], vec!["x".into(), "y".into()]]);
    let slot = |app: &Carnet, c: usize| {
        let b = blocks(app).into_iter().find(|b| matches!(b.kind, Kind::Table(_))).unwrap();
        let Kind::Table(t) = &b.kind else { unreachable!() };
        Slot { block: b.id, cell: Some((t.rows[1].id, t.cols[c])) }
    };
    app.store.notes[app.current].doc = Doc::new(vec![Block::text("Above"), pic, Block::new(Kind::Table(table)), Block::text("Below")]);
    app.caret = Caret::at(page::first(app.doc()));
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let x = slot(&app, 0);
    let rect = |app: &Carnet, id: u64| app.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r).unwrap();
    let r = rect(&app, id);
    let from = Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0);
    let cell = *app.cell_rects.get(&x).unwrap();
    let to = Vec2::new(cell.x + cell.w - 8.0, cell.y + cell.h / 2.0);

    // Over the cell: ringed, no drop line.
    d.input(&mut app, InputEvent::CursorMoved(from));
    d.input(&mut app, InputEvent::mouse_down(1));
    d.input(&mut app, InputEvent::CursorMoved(Vec2::new(from.x + 8.0, from.y + 8.0)));
    d.input(&mut app, InputEvent::CursorMoved(to));
    // Frames enough for kui's checks of the tree (every 16th) to see the
    // ring and the picture under the pointer beside the blocks.
    for _ in 0..34 {
        d.frame(&mut app);
    }
    assert!(matches!(app.pic_drag.as_ref().and_then(|p| p.target), Some(page::PicDrop::Cell { slot, .. }) if slot == x), "{:?}", app.pic_drag);
    assert_eq!(app.drop_index, None, "no drop line");
    let ringed = d.core.nodes().iter().any(|n| (n.rect.x - cell.x - 1.0).abs() < 0.5 && (n.rect.w - cell.w + 2.0).abs() < 0.5 && (n.rect.h - cell.h + 2.0).abs() < 0.5);
    assert!(ringed, "the cell ringed");
    d.input(&mut app, InputEvent::mouse_up());
    d.frame(&mut app);
    assert!(app.pic_drag.is_none());
    let md = "![](attachments/wide.png)";
    assert_eq!(page::text(app.doc(), x), format!("x{md}"), "at the end of its words");
    assert!(app.doc().get(id).is_none(), "its line gone");
    assert_eq!((app.caret.anchor.byte, app.caret.head.byte), (1, 1 + md.len()), "selected in the cell");
    key_with(&mut app, &mut d, "z", CTRL);
    assert!(app.doc().get(id).is_some() && page::text(app.doc(), x) == "x", "one undo puts it back");
    key_with(&mut app, &mut d, "z", CTRL_SHIFT);
    assert_eq!(page::text(app.doc(), x), format!("x{md}"));
    for _ in 0..4 {
        d.frame(&mut app);
    }

    // A press on it without moving: chosen, nothing moved.
    let pieces = app.cell_pieces.get(&x).expect("the cell's pieces").clone();
    let pr = d.core.layout_of(pieces.iter().find(|p| p.picture).unwrap().key).unwrap();
    let grip = Vec2::new(pr.x + pr.w / 2.0, pr.y + pr.h / 2.0);
    d.click(&mut app, grip.x, grip.y);
    d.frame(&mut app);
    assert_eq!((app.caret.anchor, app.caret.head), (Pos { slot: x, byte: 1 }, Pos { slot: x, byte: 1 + md.len() }));
    assert_eq!(page::text(app.doc(), x), format!("x{md}"));

    // To the next cell.
    let y = slot(&app, 1);
    let yc = *app.cell_rects.get(&y).unwrap();
    // Its row as tall as the picture: the room under the next cell's
    // short words is that cell's too.
    let xc = *app.cell_rects.get(&x).unwrap();
    assert!((yc.y, yc.h) == (xc.y, xc.h) && yc.h > pr.h, "{yc:?} {xc:?}");
    d.drag(&mut app, grip, Vec2::new(yc.x + yc.w / 2.0, yc.y + yc.h - 6.0));
    d.frame(&mut app);
    assert_eq!((page::text(app.doc(), x), page::text(app.doc(), y)), ("x", format!("y{md}").as_str()), "below its words: at their end");
    for _ in 0..4 {
        d.frame(&mut app);
    }

    // Out, above the first line: a line of its own, its own width.
    let pieces = app.cell_pieces.get(&y).expect("the cell's pieces").clone();
    let pr = d.core.layout_of(pieces.iter().find(|p| p.picture).unwrap().key).unwrap();
    let above = rect(&app, blocks(&app)[0].id);
    d.drag(&mut app, Vec2::new(pr.x + pr.w / 2.0, pr.y + pr.h / 2.0), Vec2::new(above.x + 40.0, above.y + 2.0));
    d.frame(&mut app);
    assert!(
        matches!(&blocks(&app)[0].kind, Kind::Image { src, width: None, float: Float::None, .. } if src == "attachments/wide.png"),
        "{:?}",
        texts(&app)
    );
    assert_eq!(page::text(app.doc(), y), "y");
    assert!(app.pic_drag.is_none() && app.drop_index.is_none());
    key_with(&mut app, &mut d, "z", CTRL);
    assert_eq!(page::text(app.doc(), y), format!("y{md}"), "one undo puts it back");
    assert!(!blocks(&app).iter().any(|b| matches!(b.kind, Kind::Image { .. })));
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A picture among a cell's words is shown chosen or with the caret
/// beside it, never both: ← from after it chooses it (its ring, no caret),
/// ← again puts the caret before it (a caret, no ring); a press chooses
/// it, with no caret. It is drawn its own width, not the one the note
/// writes for it: the cell decides.
#[test]
fn a_chosen_cell_picture_shows_its_ring_and_no_caret() {
    let (mut app, mut d, dir) = body("cell-picture-chosen");
    let rgba: Vec<u8> = (0..200 * 100).flat_map(|_| [30u8, 90, 160, 255]).collect();
    let png = crate::images::encode_png(200, 100, &rgba).unwrap();
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    std::fs::write(dir.join("attachments/wide.png"), png).unwrap();
    app.images.preload(&dir.join("attachments/wide.png"), crate::images::Decoded { w: 200, h: 100, rgba });
    let pic = "![|60](attachments/wide.png)";
    let table = Table::from_cells(vec![vec!["A".into(), "B".into()], vec![format!("x{pic}"), String::new()]]);
    app.store.notes[app.current].doc = Doc::new(vec![Block::new(Kind::Table(table)), Block::text("")]);
    let Kind::Table(t) = &blocks(&app)[0].kind else { panic!() };
    let slot = Slot { block: blocks(&app)[0].id, cell: Some((t.rows[1].id, t.cols[0])) };
    let end = 1 + pic.len();
    // After it, at the cell's end: the caret beside it.
    app.caret = Caret::at(Pos { slot, byte: end });
    d.core.set_inspect(true);
    for _ in 0..4 {
        d.frame(&mut app);
    }
    let pieces = app.cell_pieces.get(&slot).expect("a cell of pieces").clone();
    let r = d.core.layout_of(pieces.iter().find(|p| p.picture).unwrap().key).expect("the picture's box");
    assert!((r.w - 200.0).abs() <= 1.0, "its own width, not the note's 60: {r:?}");
    // The caret's bar in the cell (beside it, or after `x` above it), and
    // its ring 4px out.
    let cell = *app.cell_rects.get(&slot).unwrap();
    let near = |n: &kui_native::NodeInfo| n.rect.y >= cell.y && n.rect.y + n.rect.h <= cell.y + cell.h && n.rect.x >= cell.x && n.rect.x <= cell.x + cell.w;
    let caret = |d: &Drive| d.core.nodes().iter().any(|n| near(n) && n.rect.w == 2.0 && n.rect.h >= 8.0 && n.bg.a > 0.0);
    let ring = |d: &Drive| {
        d.core.nodes().iter().any(|n| (n.rect.w - (r.w + 8.0)).abs() < 0.5 && (n.rect.h - (r.h + 8.0)).abs() < 0.5 && n.border_w == 2.0 && n.border_color.a > 0.0)
    };
    assert!(caret(&d) && !ring(&d), "the caret, no ring");
    // ←: chosen. The ring, no caret.
    key(&mut app, &mut d, "left");
    d.frame(&mut app);
    assert_eq!((app.caret.anchor, app.caret.head), (Pos { slot, byte: end }, Pos { slot, byte: 1 }));
    assert!(ring(&d), "its ring");
    assert!(!caret(&d), "no caret beside it");
    // ← again: the caret before it, no ring.
    key(&mut app, &mut d, "left");
    d.frame(&mut app);
    assert_eq!(app.caret, Caret::at(Pos { slot, byte: 1 }));
    assert!(caret(&d) && !ring(&d), "the caret, no ring");
    // A press on it: chosen, no caret.
    d.click(&mut app, r.x + r.w / 2.0, r.y + r.h / 2.0);
    d.frame(&mut app);
    assert_eq!((app.caret.anchor, app.caret.head), (Pos { slot, byte: 1 }, Pos { slot, byte: end }));
    assert!(ring(&d) && !caret(&d), "chosen by a press: the ring alone");
    // A letter typed: in its place.
    type_in(&mut app, &mut d, "z");
    assert_eq!(page::text(app.doc(), slot), "xz");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}


/// A click on a picture in a table's cell, from anywhere — the caret in
/// another block, or the keyboard in the title — chooses it, and its ring
/// is drawn the frame the click is taken in, with no caret beside it.
#[test]
fn a_click_on_a_cell_picture_draws_its_ring_at_once() {
    let (mut app, mut d, dir) = body("cell-picture-click");
    let rgba: Vec<u8> = (0..200 * 100).flat_map(|_| [30u8, 90, 160, 255]).collect();
    let png = crate::images::encode_png(200, 100, &rgba).unwrap();
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    std::fs::write(dir.join("attachments/wide.png"), png).unwrap();
    app.images.preload(&dir.join("attachments/wide.png"), crate::images::Decoded { w: 200, h: 100, rgba });
    let pic = "![](attachments/wide.png)";
    let table = Table::from_cells(vec![vec!["A".into(), "B".into()], vec![format!("x{pic}"), String::new()]]);
    app.store.notes[app.current].doc = Doc::new(vec![Block::new(Kind::Table(table)), Block::text("after")]);
    let Kind::Table(t) = &blocks(&app)[0].kind else { panic!() };
    let slot = Slot { block: blocks(&app)[0].id, cell: Some((t.rows[1].id, t.cols[0])) };
    let end = 1 + pic.len();
    let below = Slot::block(blocks(&app)[1].id);
    d.core.set_inspect(true);
    for from in ["the paragraph below", "the title"] {
        app.caret = Caret::at(Pos { slot: below, byte: 2 });
        for _ in 0..3 {
            d.frame(&mut app);
        }
        if from == "the title" {
            app.focus(Field::Title);
            d.frame(&mut app);
            d.frame(&mut app);
        }
        let pieces = app.cell_pieces.get(&slot).expect("a cell of pieces").clone();
        let r = d.core.layout_of(pieces.iter().find(|p| p.picture).unwrap().key).expect("the picture's box");
        let cell = *app.cell_rects.get(&slot).unwrap();
        let near = |n: &kui_native::NodeInfo| n.rect.y >= cell.y && n.rect.y + n.rect.h <= cell.y + cell.h && n.rect.x >= cell.x && n.rect.x <= cell.x + cell.w;
        let caret = |d: &Drive| d.core.nodes().iter().any(|n| near(n) && n.rect.w == 2.0 && n.rect.h >= 8.0 && n.bg.a > 0.0);
        let ring = |d: &Drive| {
            d.core.nodes().iter().any(|n| (n.rect.w - (r.w + 8.0)).abs() < 0.5 && (n.rect.h - (r.h + 8.0)).abs() < 0.5 && n.border_w == 2.0 && n.border_color.a > 0.0)
        };
        // One click, and the one frame it is taken in.
        d.click(&mut app, r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert_eq!((app.caret.anchor, app.caret.head), (Pos { slot, byte: 1 }, Pos { slot, byte: end }), "from {from}");
        assert!(page_focused(&app, &d), "from {from}: the keyboard on the page");
        assert!(ring(&d), "from {from}: its ring, that frame");
        assert!(!caret(&d), "from {from}: no caret beside it");
    }
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}


/// A divider chosen draws one way however it was come to — by ← or by ↑:
/// the ornament in the accent on the selection's wash, the wash as wide
/// as the text (a table's width, no wider) and a band round the ornament,
/// not its whole tall row.
#[test]
fn a_chosen_divider_is_washed_to_the_texts_width_whichever_arrow_chose_it() {
    let (mut app, mut d, dir) = body("rule-chosen");
    app.edit(Step::Other, |doc, c| {
        *doc = carnet_core::doc::Doc::new(vec![
            Block::text("Above"),
            Block::new(Kind::Table(carnet_core::doc::Table::new(2, 2))),
            Block::new(Kind::Divider),
            Block::text("Below"),
        ]);
        *c = Caret::at(page::start(Slot::block(doc.blocks()[3].id)));
        true
    });
    d.core.set_inspect(true);
    for _ in 0..3 {
        d.frame(&mut app);
    }
    let rule = blocks(&app)[2].id;
    let row = |app: &Carnet, id: u64| app.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r).unwrap();
    let wash = |d: &Drive| {
        let sel = d.core.theme().selection;
        let all: Vec<Rect> = d.core.nodes().iter().filter(|n| n.bg == sel && n.rect.w > 0.0).map(|n| n.rect).collect();
        assert_eq!(all.len(), 1, "one wash: {all:?}");
        all[0]
    };
    // ↑ from the line below: the divider chosen whole, as ← chooses it.
    key(&mut app, &mut d, "up");
    assert_eq!(page::chosen_atom(app.doc(), app.caret), Some(Slot::block(rule)));
    d.frame(&mut app);
    let by_up = wash(&d);
    // The next ↓ is past it; ← from there chooses it again.
    key(&mut app, &mut d, "down");
    assert_eq!(app.caret, Caret::at(page::start(Slot::block(blocks(&app)[3].id))));
    key(&mut app, &mut d, "left");
    assert_eq!(page::chosen_atom(app.doc(), app.caret), Some(Slot::block(rule)));
    d.frame(&mut app);
    let by_left = wash(&d);
    assert_eq!(by_up, by_left, "one way, whichever arrow");
    // As wide as the text, and the table: the rows' own padding inside.
    let (words, table, tall) = (row(&app, blocks(&app)[0].id), row(&app, blocks(&app)[1].id), row(&app, rule));
    for r in [words, table] {
        assert!(by_up.x >= r.x + 8.0 - 0.5 && by_up.x + by_up.w <= r.x + r.w - 8.0 + 0.5, "no wider than the text: {r:?} {by_up:?}");
        assert!((by_up.w - (r.w - 16.0)).abs() < 1.0, "the text's width: {r:?} {by_up:?}");
    }
    assert!(by_up.h < tall.h * 0.7, "a band round the ornament, not its row: {tall:?} {by_up:?}");
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Words typed over a picture the words run beside, or over a divider,
/// chosen by the arrows, make a paragraph of their own in its place, as
/// carnet-core has it: the checklist or quote after it keeps its words.
/// So whether typed, typed with Bold switched on (Ctrl+B with it chosen:
/// the words in its place bold), or pasted (Ctrl+V and the text kui hands
/// over).
#[test]
fn words_over_a_chosen_float_or_divider_are_a_paragraph_of_their_own() {
    let (mut app, mut d, dir) = body("over-atom");
    let note = || {
        carnet_core::doc::Doc::new(vec![
            Block::text("Plan"),
            Block::new(Kind::Image { src: "a.png".into(), alt: String::new(), width: Some(200), float: Float::Left, wiki: false }),
            Block::new(Kind::Item { style: ListStyle::Todo, indent: 0, done: true, num: 0, text: "Book".into() }),
            Block::text("Then"),
            Block::new(Kind::Divider),
            Block::new(Kind::Quote("The light".into())),
            Block::text(""),
        ])
    };
    for how in ["typed", "bold", "pasted"] {
        // The picture chosen by → from the end of the line above it, then
        // the divider the same way.
        for (above, atom) in [(0, 1), (3, 4)] {
            app.edit(Step::Other, |doc, c| {
                *doc = note();
                *c = Caret::at(page::end(doc, Slot::block(doc.blocks()[above].id)));
                true
            });
            d.frame(&mut app);
            key(&mut app, &mut d, "right");
            let id = blocks(&app)[atom].id;
            assert_eq!(page::chosen_atom(app.doc(), app.caret), Some(Slot::block(id)), "{how}");
            match how {
                "typed" => type_in(&mut app, &mut d, "replaced"),
                "bold" => {
                    key_with(&mut app, &mut d, "b", CTRL);
                    type_in(&mut app, &mut d, "replaced");
                }
                _ => {
                    key_with(&mut app, &mut d, "v", CTRL);
                    d.input(&mut app, InputEvent::Paste { text: "replaced".into(), marks: Default::default() });
                    d.frame(&mut app);
                }
            }
            let words = if how == "bold" { "**replaced**" } else { "replaced" };
            let mut want: Vec<String> = ["Plan", "", "[x]Book", "Then", "---", "The light", ""].map(String::from).to_vec();
            want[atom] = words.into();
            assert_eq!(texts(&app), want, "{how}: a paragraph in its place, the words after as they were");
            assert!(matches!(&blocks(&app)[atom].kind, Kind::Text(_)), "{how}");
            assert!(blocks(&app).iter().all(|b| b.id != id), "{how}: the picture or divider gone");
            // The caret after the words, inside the bold's marks.
            let byte = if how == "bold" { 10 } else { 8 };
            assert_eq!(app.caret, Caret::at(Pos { slot: Slot::block(blocks(&app)[atom].id), byte }), "{how}");
        }
    }
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A library of folders and notes written out, opened: `name` its own
/// folder under the temp dir; each note `# leaf` and its words.
fn library(name: &str, notes: &[(&str, &str)]) -> (Carnet, Drive, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("carnet-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, body) in notes {
        let file = dir.join(format!("{path}.md"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let leaf = path.rsplit('/').next().unwrap();
        std::fs::write(file, format!("# {leaf}\n\n{body}\n")).unwrap();
    }
    let mut app = Carnet::new(Store::open(dir.clone()).unwrap());
    let mut d = Drive::new(Core::new(), 1100.0, 760.0).framing();
    d.core.set_inspect(true);
    d_frame_twice(&mut app, &mut d);
    (app, d, dir)
}

/// Where folder `f`'s row is ("" for All notes).
fn folder_row(app: &Carnet, f: &str) -> Vec2 {
    app.folder_rects.iter().find(|(r, _)| r == f).map(|(_, r)| r.center()).unwrap_or_else(|| panic!("no row for {f:?}"))
}

/// A folder's row dragged onto another's, the library drawn again.
fn drag_folder(app: &mut Carnet, d: &mut Drive, from: &str, to: &str) {
    let (a, b) = (folder_row(app, from), folder_row(app, to));
    d.drag(app, a, b);
    d_frame_twice(app, d);
}

/// A folder dragged onto another goes inside it, its notes and
/// subfolders with it; onto "All notes", to the top level. Its notes'
/// links to pictures outside it follow it, a level deeper and back.
#[test]
fn a_folder_dragged_onto_another_goes_inside_it_and_onto_all_notes_to_the_top() {
    let (mut app, mut d, dir) = library(
        "drag-folder",
        &[("Work/Plan", "![](../attachments/p.png)"), ("Work/Sub/Deep", "deep"), ("Home/Pantry", "rice")],
    );
    let png = png_file(&dir.join("attachments"), "p.png");
    assert!(png.exists());
    drag_folder(&mut app, &mut d, "Work", "Home");
    assert!(dir.join("Home/Work/Plan.md").exists() && dir.join("Home/Work/Sub/Deep.md").exists(), "moved on disk");
    assert!(!dir.join("Work").exists(), "nothing left behind");
    assert!(app.store.folders.contains(&"Home/Work".to_string()) && app.store.folders.contains(&"Home/Work/Sub".to_string()), "{:?}", app.store.folders);
    let plan = std::fs::read_to_string(dir.join("Home/Work/Plan.md")).unwrap();
    assert!(plan.contains("](../../attachments/p.png)"), "its picture's link from its new place: {plan}");
    assert_eq!(app.notice.as_ref().map(|n| n.text.as_str()), Some("Moved \u{201c}Work\u{201d} to \u{201c}Home\u{201d}"));
    // Onto All notes: the top level again, the link as it was.
    drag_folder(&mut app, &mut d, "Home/Work", "");
    assert!(dir.join("Work/Plan.md").exists() && dir.join("Work/Sub/Deep.md").exists() && !dir.join("Home/Work").exists());
    let plan = std::fs::read_to_string(dir.join("Work/Plan.md")).unwrap();
    assert!(plan.contains("](../attachments/p.png)"), "{plan}");
    // A plain click on a folder's row still shows it.
    let at = folder_row(&app, "Home");
    d.click(&mut app, at.x, at.y);
    d.frame(&mut app);
    assert_eq!(app.folder.as_deref(), Some("Home"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A folder never goes into itself or a folder inside it: let go there,
/// nothing moves.
#[test]
fn a_folder_dragged_into_its_own_subfolder_stays() {
    let (mut app, mut d, dir) = library("drag-folder-self", &[("Work/Plan", "q3"), ("Work/Sub/Deep", "deep")]);
    drag_folder(&mut app, &mut d, "Work", "Work/Sub");
    assert!(dir.join("Work/Plan.md").exists() && dir.join("Work/Sub/Deep.md").exists(), "where it was");
    assert_eq!(app.store.folders, ["Work", "Work/Sub"]);
    assert!(app.error.is_none(), "{:?}", app.error);
    // Its own row: the same.
    let (a, b) = (folder_row(&app, "Work"), folder_row(&app, "Work"));
    d.drag(&mut app, a, Vec2::new(b.x + 20.0, b.y));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.store.folders, ["Work", "Work/Sub"]);
    // The store refuses it too.
    assert!(app.store.move_folder("Work", "Work/Sub").is_err());
    let _ = std::fs::remove_dir_all(dir);
}

/// Several chosen, a folder and a note: dragging the folder carries both.
#[test]
fn several_chosen_are_dragged_into_a_folder_together() {
    let (mut app, mut d, dir) = library("drag-picked", &[("Home/Pantry", "rice"), ("Loose", "words"), ("Dest/Kept", "k")]);
    let i = app.store.notes.iter().position(|n| n.title == "Loose").unwrap();
    app.open(i);
    d_frame_twice(&mut app, &mut d);
    let cmd = KeyMods { shift: false, ctrl: !cfg!(target_os = "macos"), alt: false, super_key: cfg!(target_os = "macos") };
    let home = folder_row(&app, "Home");
    d.input(&mut app, InputEvent::Modifiers(cmd));
    d.click(&mut app, home.x, home.y);
    d.input(&mut app, InputEvent::Modifiers(KeyMods::NONE));
    d.frame(&mut app);
    let loose = app.store.notes[i].id;
    assert!(app.is_picked(&Pick::Note(loose)) && app.is_picked(&Pick::Folder("Home".into())), "{:?}", app.picked);
    drag_folder(&mut app, &mut d, "Home", "Dest");
    assert!(dir.join("Dest/Home/Pantry.md").exists() && dir.join("Dest/Loose.md").exists(), "both moved");
    assert!(!dir.join("Home").exists() && !dir.join("Loose.md").exists());
    assert!(app.picked.is_empty());
    assert_eq!(app.notice.as_ref().map(|n| n.text.as_str()), Some("Moved a folder and a note to \u{201c}Dest\u{201d}"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A choice with a folder in it is offered "Move … to" in its menu too,
/// never into one of the chosen folders or one inside them; chosen, it
/// moves them all.
#[test]
fn chosen_folders_and_notes_move_by_the_menu_too() {
    let (mut app, mut d, dir) = library("menu-picked", &[("Home/Pantry", "rice"), ("Home/Deep/Low", "x"), ("Loose", "words"), ("Dest/Kept", "k")]);
    let i = app.store.notes.iter().position(|n| n.title == "Loose").unwrap();
    app.open(i);
    d_frame_twice(&mut app, &mut d);
    let cmd = KeyMods { shift: false, ctrl: !cfg!(target_os = "macos"), alt: false, super_key: cfg!(target_os = "macos") };
    let home = folder_row(&app, "Home");
    d.input(&mut app, InputEvent::Modifiers(cmd));
    d.click(&mut app, home.x, home.y);
    d.input(&mut app, InputEvent::Modifiers(KeyMods::NONE));
    d.frame(&mut app);
    let menu = app.picked_menu();
    let to = menu.iter().find(|m| m.label == "Move a folder and a note to").expect("offered");
    let targets: Vec<&str> = to.submenu.iter().map(|m| m.label.as_str()).collect();
    assert!(targets.contains(&"To the top") && targets.contains(&"Dest"), "{targets:?}");
    assert!(!targets.iter().any(|t| t.starts_with("Home")), "never into itself: {targets:?}");
    app.move_picked("Dest");
    d.frame(&mut app);
    assert!(dir.join("Dest/Home/Pantry.md").exists() && dir.join("Dest/Home/Deep/Low.md").exists() && dir.join("Dest/Loose.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// What is chosen in the library, by name: a folder's path with a `/`
/// after it, a note's title; in order.
fn picked_names(app: &Carnet) -> Vec<String> {
    let mut v: Vec<String> = app
        .picked
        .iter()
        .map(|p| match p {
            Pick::Note(id) => app.store.notes.iter().find(|n| n.id == *id).unwrap().title.clone(),
            Pick::Folder(f) => format!("{f}/"),
        })
        .collect();
    v.sort();
    v
}

/// ⇧↓ from a folder chooses it and the next, and again the one after;
/// ⇧↑ gives the last back. A plain ↓ lets go and moves on, Esc lets go,
/// and Delete takes what is chosen to the trash, Undo bringing it back.
#[test]
fn shift_with_the_arrows_chooses_folders_from_the_keyboard() {
    let (mut app, mut d, dir) = library("shift-folders", &[("A/a", "1"), ("B/b", "2"), ("C/c", "3"), ("D/d", "4")]);
    let at = folder_row(&app, "A");
    d.click(&mut app, at.x, at.y);
    d.frame(&mut app);
    assert_eq!(app.folder.as_deref(), Some("A"));
    key_with(&mut app, &mut d, "down", SHIFT);
    key_with(&mut app, &mut d, "down", SHIFT);
    assert_eq!(picked_names(&app), ["A/", "B/", "C/"]);
    assert_eq!(app.folder.as_deref(), Some("A"), "the folder shown stays");
    key_with(&mut app, &mut d, "up", SHIFT);
    assert_eq!(picked_names(&app), ["A/", "B/"], "back toward where it began");
    key_with(&mut app, &mut d, "down", SHIFT);
    assert_eq!(picked_names(&app), ["A/", "B/", "C/"]);
    // Esc lets go; a plain ↓ moves on from where the keyboard is.
    key(&mut app, &mut d, "escape");
    assert!(app.picked.is_empty());
    key_with(&mut app, &mut d, "down", SHIFT);
    assert_eq!(picked_names(&app), ["C/", "D/"], "from the row the keyboard was on");
    key(&mut app, &mut d, "up");
    assert!(app.picked.is_empty(), "a plain arrow lets go");
    assert_eq!(app.folder.as_deref(), Some("C"), "and moves");
    // From there up, then to the trash together; Undo brings them back.
    key_with(&mut app, &mut d, "up", SHIFT);
    key_with(&mut app, &mut d, "up", SHIFT);
    assert_eq!(picked_names(&app), ["A/", "B/", "C/"]);
    key(&mut app, &mut d, "delete");
    d_frame_twice(&mut app, &mut d);
    assert!(app.picked.is_empty());
    assert_eq!(app.store.folders, ["D"]);
    assert!(!dir.join("A").exists() && !dir.join("B").exists() && !dir.join("C").exists() && dir.join("D/d.md").exists());
    assert_eq!(app.notice.as_ref().map(|n| n.text.as_str()), Some("Moved 3 folders to Deleted"));
    app.on_plain(Msg::Undo);
    d.frame(&mut app);
    assert!(dir.join("A/a.md").exists() && dir.join("B/b.md").exists() && dir.join("C/c.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// The same in the notes: after a click on a card, ⇧↓ chooses the notes
/// below it, ⇧↑ gives one back, a plain ↓ opens the next; Delete takes
/// what is chosen to the trash, and Undo brings it back.
#[test]
fn shift_with_the_arrows_chooses_notes_from_the_keyboard() {
    let (mut app, mut d, dir) = library("shift-notes", &[("Alpha", "1"), ("Bravo", "2"), ("Charlie", "3"), ("Delta", "4")]);
    app.on_settings(Msg::SetSort { to: 1 });
    d_frame_twice(&mut app, &mut d);
    let card = |app: &Carnet, t: &str| {
        let i = app.store.notes.iter().position(|n| n.title == t).unwrap();
        app.card_rects.iter().find(|(n, _)| *n == i).map(|(_, r)| r.center()).unwrap()
    };
    let titles: Vec<String> = app.order.iter().map(|&i| app.store.notes[i].title.clone()).collect();
    let alpha = titles.iter().position(|t| t == "Alpha").unwrap();
    assert_eq!(titles[alpha..alpha + 4], ["Alpha", "Bravo", "Charlie", "Delta"], "{titles:?}");
    let at = card(&app, "Alpha");
    d.click(&mut app, at.x, at.y);
    d.frame(&mut app);
    assert_eq!(app.store.notes[app.current].title, "Alpha");
    key_with(&mut app, &mut d, "down", SHIFT);
    key_with(&mut app, &mut d, "down", SHIFT);
    assert_eq!(picked_names(&app), ["Alpha", "Bravo", "Charlie"]);
    key_with(&mut app, &mut d, "up", SHIFT);
    assert_eq!(picked_names(&app), ["Alpha", "Bravo"]);
    assert_eq!(app.store.notes[app.current].title, "Alpha", "the open note stays open");
    // A plain ↓ lets go and opens the next.
    key(&mut app, &mut d, "down");
    assert!(app.picked.is_empty());
    assert_eq!(app.store.notes[app.current].title, "Bravo");
    // From there, two chosen, and to the trash; Undo brings them back.
    key_with(&mut app, &mut d, "down", SHIFT);
    assert_eq!(picked_names(&app), ["Bravo", "Charlie"]);
    key(&mut app, &mut d, "backspace");
    d_frame_twice(&mut app, &mut d);
    assert!(!dir.join("Bravo.md").exists() && !dir.join("Charlie.md").exists() && dir.join("Delta.md").exists());
    assert_eq!(app.notice.as_ref().map(|n| n.text.as_str()), Some("Moved 2 notes to Deleted"));
    app.on_plain(Msg::Undo);
    d.frame(&mut app);
    assert!(dir.join("Bravo.md").exists() && dir.join("Charlie.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// A note another app writes into the folder shows in the list while the
/// window stays where it is: no coming to the front needed. Not while
/// typing here, though: once it pauses.
#[test]
fn a_file_written_from_outside_shows_without_the_window_coming_to_the_front() {
    let (mut app, mut d, dir) = body("outside");
    type_in(&mut app, &mut d, "typing");
    std::fs::write(dir.join("From outside.md"), "# From outside\n\nwritten by another app\n").unwrap();
    let shown = |app: &Carnet| app.store.notes.iter().any(|n| n.title == "From outside");
    let t0 = Instant::now();
    while !shown(&app) && t0.elapsed() < Duration::from_secs(10) {
        if t0.elapsed() < WATCH_QUIET / 2 {
            type_in(&mut app, &mut d, "a");
        }
        std::thread::sleep(Duration::from_millis(20));
        d.frame(&mut app);
    }
    assert!(shown(&app), "not seen in {:?}", t0.elapsed());
    assert!(t0.elapsed() >= WATCH_QUIET, "waited for the typing to pause: {:?}", t0.elapsed());
    assert!(app.order.iter().any(|&i| app.store.notes[i].title == "From outside"), "and listed");
    let _ = std::fs::remove_dir_all(dir);
}

/// A note saved here is no news to the folder's watch: the save is the
/// app's own, and no rescan follows it. One written by another app is.
#[test]
fn an_autosave_brings_no_rescan_and_another_apps_write_does() {
    let (mut app, mut d, dir) = body("own-save");
    let run = |app: &mut Carnet, d: &mut Drive, secs: u64, until: &dyn Fn(&Carnet) -> bool| {
        let t0 = Instant::now();
        while !until(app) && t0.elapsed() < Duration::from_secs(secs) {
            std::thread::sleep(Duration::from_millis(20));
            d.frame(app);
        }
    };
    // The start's look, once typing has paused.
    run(&mut app, &mut d, 10, &|a| a.rescans > 0);
    assert_eq!(app.rescans, 1, "the start counts once");
    type_in(&mut app, &mut d, "Saved by itself");
    run(&mut app, &mut d, 10, &|a| !a.store.notes[a.current].dirty && a.store.notes[a.current].path.is_some());
    let path = app.store.notes[app.current].path.clone().expect("saved");
    assert!(std::fs::read_to_string(&path).unwrap().contains("Saved by itself"));
    // Past the pause a change waits for, and the system's word.
    run(&mut app, &mut d, 4, &|_| false);
    assert_eq!(app.rescans, 1, "the app's own save: no rescan");
    std::fs::write(&path, "# Saved by itself\n\nand edited elsewhere\n").unwrap();
    run(&mut app, &mut d, 10, &|a| a.rescans > 1);
    assert_eq!(app.rescans, 2, "another app's edit: one");
    assert!(app.store.notes.iter().any(|n| n.markdown().contains("edited elsewhere")));
    let _ = std::fs::remove_dir_all(dir);
}


/// A press in the title held over a frame or two, as a hand holds it,
/// and let go: where it was let go, the title's caret.
fn press_title(app: &mut Carnet, d: &mut Drive, at: f32) -> Key {
    let k = app_field(app, Field::Title).expect("the title");
    let r = d.rect_of(k).unwrap();
    d.input(app, InputEvent::CursorMoved(Vec2::new(r.x + at.clamp(2.0, r.w - 2.0), r.y + r.h / 2.0)));
    d.input(app, InputEvent::mouse_down(1));
    d.frame(app);
    d.frame(app);
    d.input(app, InputEvent::mouse_up());
    d.frame(app);
    assert_eq!(d.core.focus(), Some(k), "the title has the keyboard");
    k
}

/// After a click in the title, its first key acts: Backspace takes a
/// letter, ← at the start and → at the end go into the note's start (→)
/// or stay (←, there being nothing before the title), ↓ goes into the
/// note — not the second press, as when kui's press left the field an
/// empty selection, which has no edge and is what Backspace took.
#[test]
fn the_first_key_after_a_click_in_the_title_acts() {
    let (mut app, mut d, dir) = fresh("title-click-key");
    d.text(&mut app, "Plans");
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "First line");
    // Backspace at the end: a letter goes.
    let k = press_title(&mut app, &mut d, 1e4);
    key(&mut app, &mut d, "backspace");
    assert_eq!(d.core.edit_text(k).as_deref(), Some("Plan"));
    assert_eq!(app.store.notes[app.current].title, "Plan");
    // → at the end: into the note, at its start.
    click_page(&mut app, &mut d);
    press_title(&mut app, &mut d, 1e4);
    key(&mut app, &mut d, "right");
    d.frame(&mut app);
    assert!(page_focused(&app, &d), "→ at the title's end is the note");
    assert_eq!(app.caret, Caret::at(page::first(app.doc())));
    // ↓ from the middle: into the note.
    press_title(&mut app, &mut d, 30.0);
    key(&mut app, &mut d, "down");
    d.frame(&mut app);
    assert!(page_focused(&app, &d), "↓ from the title is the note");
    // ← from the middle: a letter left, at once.
    let k = press_title(&mut app, &mut d, 30.0);
    let at = d.core.edit.selection_cursors(k).unwrap().1;
    key(&mut app, &mut d, "left");
    assert_eq!(d.core.edit.selection_cursors(k).unwrap().1 .1 + 1, at.1);
    // A drag in the title still selects, and Backspace takes what it did.
    let r = d.rect_of(k).unwrap();
    d.drag(&mut app, Vec2::new(r.x + 2.0, r.y + r.h / 2.0), Vec2::new(r.x + r.w - 2.0, r.y + r.h / 2.0));
    d.frame(&mut app);
    assert!(d.core.edit.has_selection(k));
    key(&mut app, &mut d, "backspace");
    assert_eq!(d.core.edit_text(k).as_deref(), Some(""));
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A picture straight after the title, chosen by ↓ or → from the title's
/// end, is typed over as one with a paragraph above it is: the words take
/// its place, a paragraph of their own.
#[test]
fn a_picture_straight_after_the_title_is_typed_over() {
    for how in ["down", "right"] {
        let (mut app, mut d, dir) = library(&format!("pic-after-title-{how}"), &[("Title", "![x|left|220](attachments/x.png)\n\n- [ ] Book")]);
        png_file(&dir.join("attachments"), "x.png");
        app.open(app.store.notes.iter().position(|n| n.title == "Title").unwrap());
        d_frame_twice(&mut app, &mut d);
        assert!(matches!(&blocks(&app)[0].kind, Kind::Image { float: Float::Left, width: Some(220), .. }), "{how}: {:?}", blocks(&app)[0].kind);
        let pic = blocks(&app)[0].id;
        press_title(&mut app, &mut d, 1e4);
        key(&mut app, &mut d, how);
        d.frame(&mut app);
        assert!(page_focused(&app, &d), "{how}");
        assert_eq!(page::chosen_atom(app.doc(), app.caret), Some(Slot::block(pic)), "{how}: the picture chosen");
        type_in(&mut app, &mut d, "replaced");
        assert_eq!(texts(&app)[..2], ["replaced", "[ ]Book"], "{how}: a paragraph in its place");
        assert!(blocks(&app).iter().all(|b| b.id != pic), "{how}: the picture gone");
        assert!(matches!(&blocks(&app)[0].kind, Kind::Text(_)), "{how}");
        app.save_current();
        let md = std::fs::read_to_string(dir.join("Title.md")).unwrap();
        assert!(md.starts_with("# Title\n\nreplaced\n\n- [ ] Book\n"), "{how}: {md:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The open note's `# ` line changed from outside: the page draws the new
/// title, as the list does, and still after another note and back.
#[test]
fn a_title_changed_from_outside_is_drawn() {
    let (mut app, mut d, dir) = library("outside-title", &[("Plans", "words"), ("Other", "more")]);
    let at = |app: &Carnet, t: &str| app.store.notes.iter().position(|n| n.title == t);
    app.open(at(&app, "Plans").unwrap());
    d_frame_twice(&mut app, &mut d);
    let drawn_title = |app: &Carnet, d: &Drive| app_field(app, Field::Title).and_then(|k| d.core.edit_text(k));
    assert_eq!(drawn_title(&app, &d).as_deref(), Some("Plans"));
    let id = app.store.notes[app.current].id;
    std::fs::write(dir.join("Plans.md"), "# Plans for June\n\nwords, and more\n").unwrap();
    let t0 = Instant::now();
    while app.store.notes[app.current].title != "Plans for June" && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
        d.frame(&mut app);
    }
    d_frame_twice(&mut app, &mut d);
    assert_eq!(app.store.notes[app.current].id, id, "the same note open");
    assert!(app.store.notes[app.current].markdown().contains("words, and more"), "the body reloaded");
    assert_eq!(drawn_title(&app, &d).as_deref(), Some("Plans for June"), "and the title drawn");
    // Another note, and back: still the new title.
    app.open(at(&app, "Other").unwrap());
    d_frame_twice(&mut app, &mut d);
    assert_eq!(drawn_title(&app, &d).as_deref(), Some("Other"));
    app.open(at(&app, "Plans for June").unwrap());
    d_frame_twice(&mut app, &mut d);
    assert_eq!(drawn_title(&app, &d).as_deref(), Some("Plans for June"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A folder chosen by a click goes with Backspace or Delete, as one with
/// the keyboard on its row does; on a Mac ⌘Backspace too, on the row or
/// after the click.
#[test]
fn backspace_or_delete_after_a_click_on_a_folder_removes_it() {
    let (mut app, mut d, dir) = on_folder_row("row-click-backspace", &["A", "B", "C", "D"]);
    let click = |app: &mut Carnet, d: &mut Drive, f: &str| {
        let k = app.folder_rows.iter().find(|(_, r)| *r == folder(f)).unwrap().0;
        let r = d.rect_of(k).unwrap();
        d.click(app, r.x + 20.0, r.y + r.h / 2.0);
        d.frame(app);
        assert_eq!(app.folder.as_deref(), Some(f));
    };
    click(&mut app, &mut d, "B");
    key(&mut app, &mut d, "backspace");
    assert_eq!(app.store.folders, ["A", "C", "D"]);
    assert_eq!(focused_row(&app, &d), Some(folder("C")), "the keyboard on the row that took its place");
    click(&mut app, &mut d, "A");
    key(&mut app, &mut d, "delete");
    assert_eq!(app.store.folders, ["C", "D"]);
    if cfg!(target_os = "macos") {
        click(&mut app, &mut d, "C");
        key_with(&mut app, &mut d, "backspace", CMD);
        assert_eq!(app.store.folders, ["D"]);
        focus_row(&mut app, &mut d, folder("D"));
        key_with(&mut app, &mut d, "backspace", CMD);
        assert!(app.store.folders.is_empty());
    }
    // Nothing shown but all notes: nothing goes, and no note either.
    let notes = app.store.notes.len();
    app.show_folder(None);
    d.core.set_focus(None);
    d.frame(&mut app);
    key(&mut app, &mut d, "backspace");
    assert_eq!(app.store.notes.len(), notes);
    let _ = std::fs::remove_dir_all(dir);
}

/// A right-click on the library's row whose words are `row`, then a click
/// on the row `choice` of the menu it opened.
fn row_menu(app: &mut Carnet, d: &mut Drive, row: &str, choice: &str) {
    d.core.set_inspect(true);
    d.frame(app);
    let edge = app.library_w();
    let at = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some(row) && n.rect.x < edge)
        .map(|n| n.rect.center())
        .unwrap_or_else(|| panic!("{row} is drawn in the library"));
    d.input(app, InputEvent::CursorMoved(at));
    d.input(app, InputEvent::MouseDown { button: kui_native::MouseButton::Secondary, clicks: 1 });
    d.input(app, InputEvent::MouseUp { button: kui_native::MouseButton::Secondary });
    d_frame_twice(app, d);
    let at = d
        .core
        .nodes()
        .iter()
        .find(|n| n.text.as_deref() == Some(choice))
        .map(|n| n.rect.center())
        .unwrap_or_else(|| panic!("{choice} is in the menu"));
    d.click(app, at.x, at.y);
    d.frame(app);
}

/// A folder's menu reaches the app (kui posts a choice only to a node that
/// declares where it goes), and so do its name field's words, Enter and
/// Esc.
#[test]
fn a_folders_menu_renames_it_and_esc_lets_the_name_go() {
    let (mut app, mut d, dir) = fresh("folder-menu");
    app.store.create_folder("", "Work").unwrap();
    d.frame(&mut app);
    row_menu(&mut app, &mut d, "Work", "Rename\u{2026}");
    assert_eq!(app.naming.as_ref().and_then(|n| n.rename.as_deref()), Some("Work"));
    d.frame(&mut app);
    let field = d.core.focus().and_then(|k| app.fields.get(&k)).copied();
    assert_eq!(field, Some(Field::Folder), "the name's field has the keyboard");
    type_in(&mut app, &mut d, "s");
    key(&mut app, &mut d, "enter");
    assert!(app.store.folders.iter().any(|f| f == "Works"), "{:?}", app.store.folders);
    assert!(app.naming.is_none());
    // Again, and let go of.
    row_menu(&mut app, &mut d, "Works", "Rename\u{2026}");
    d.frame(&mut app);
    type_in(&mut app, &mut d, "hop");
    key(&mut app, &mut d, "escape");
    assert!(app.naming.is_none(), "Esc drops the name");
    assert_eq!(app.store.folders, ["Works"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// A row of the Deleted folder's menu reaches the app.
#[test]
fn a_deleted_rows_menu_puts_it_back() {
    let (mut app, mut d, dir) = fresh("deleted-menu");
    d.text(&mut app, "Old plans");
    d.frame(&mut app);
    app.delete_note_at(app.current);
    app.notice = None;
    app.on_plain(Msg::ShowDeleted);
    d.frame(&mut app);
    row_menu(&mut app, &mut d, "Old plans", "Restore");
    assert!(app.trash.iter().all(|t| t.title != "Old plans"), "out of the trash");
    assert!(app.store.notes.iter().any(|n| n.display_title() == "Old plans"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A row of the menu bar the app runs itself, not a key kui plays back.
#[test]
fn a_bar_row_the_app_runs_reaches_it() {
    let (mut app, mut d, dir) = fresh_mac("menubar-own-row");
    let was = app.settings.focus;
    choose(&mut app, &mut d, "View", "Focus");
    assert_eq!(app.settings.focus, !was);
    let _ = std::fs::remove_dir_all(dir);
}

/// A kui field shows the model's words on every frame (its `value`): a
/// title another device changed, with the keyboard in it or not, and a
/// search the app emptied. What is typed goes on from there.
#[test]
fn a_field_shows_what_the_model_holds() {
    let (mut app, mut d, dir) = fresh("field-value");
    type_in(&mut app, &mut d, "Mine");
    let title = app_field(&app, Field::Title).expect("the title's field");
    assert_eq!(d.core.focus(), Some(title));
    app.store.notes[app.current].title = "Theirs".into();
    d.frame(&mut app);
    assert_eq!(d.core.edit_text(title).as_deref(), Some("Theirs"), "while it has the keyboard");
    type_in(&mut app, &mut d, "!");
    assert_eq!(app.store.notes[app.current].title, "Theirs!", "typing goes on at its end");
    key(&mut app, &mut d, "enter");
    assert!(page_focused(&app, &d));
    app.store.notes[app.current].title = "Elsewhere".into();
    d.frame(&mut app);
    assert_eq!(d.core.edit_text(title).as_deref(), Some("Elsewhere"), "and without it");
    // The search, emptied by the app: so is its field.
    key_with(&mut app, &mut d, "f", CTRL);
    type_in(&mut app, &mut d, "zzz");
    let search = app_field(&app, Field::Search).expect("the search");
    assert_eq!(d.core.edit_text(search).as_deref(), Some("zzz"));
    app.on_plain(Msg::ClearSearch);
    d.frame(&mut app);
    assert_eq!(d.core.edit_text(search).as_deref(), Some(""));
    let _ = std::fs::remove_dir_all(dir);
}

/// Settings → Sync's address put in by the app (the last server signed
/// in to, say) shows in its field, with nothing to tell the field.
#[test]
fn the_sync_address_the_app_fills_shows_in_its_field() {
    let (mut app, mut d, dir) = tall("sync-address-value");
    let field = app_field(&app, Field::SyncServer).expect("the server's field");
    assert_eq!(d.core.edit_text(field).as_deref(), Some(""));
    app.sync_server = "https://notes.example.com".into();
    d.frame(&mut app);
    assert_eq!(d.core.edit_text(field).as_deref(), Some("https://notes.example.com"));
    let _ = std::fs::remove_dir_all(dir);
}

/// The password typed to sign in goes with its field: Back to the ways in,
/// or Settings closed, and the form opens again with none — in the field
/// or kept by the app.
#[test]
fn a_sign_in_password_left_behind_is_forgotten() {
    let (store, dir) = temp_store("pw-forgotten");
    let mut app = Carnet::new(store);
    let mut d = Drive::new(Core::new(), 1100.0, 1600.0).framing();
    app.settings_open = true;
    app.connect.assume("https://notes.example.com", server_info(&["passwords"]));
    let typed = |app: &mut Carnet, d: &mut Drive| {
        app.on_plain(Msg::SyncHow { to: 2 });
        d_frame_twice(app, d);
        let k = app_field(app, Field::SyncPassword).expect("the password's field");
        d.focus(app, k);
        d.text(app, "hunter22");
        d_frame_twice(app, d);
        assert_eq!(app.sync_password, "hunter22");
        k
    };
    let reopened = |app: &mut Carnet, d: &mut Drive| {
        app.on_plain(Msg::SyncHow { to: 2 });
        d_frame_twice(app, d);
        let k = app_field(app, Field::SyncPassword).expect("the password's field");
        (app.sync_password.clone(), d.core.edit_text(k).unwrap_or_default())
    };
    typed(&mut app, &mut d);
    app.on_plain(Msg::SyncBack);
    d_frame_twice(&mut app, &mut d);
    assert!(app.sync_password.is_empty(), "Back: forgotten");
    assert_eq!(reopened(&mut app, &mut d), (String::new(), String::new()));
    typed(&mut app, &mut d);
    app.settings_open = false;
    d_frame_twice(&mut app, &mut d);
    app.settings_open = true;
    d_frame_twice(&mut app, &mut d);
    assert_eq!(reopened(&mut app, &mut d), (String::new(), String::new()), "Settings closed: forgotten");
    let _ = std::fs::remove_dir_all(dir);
}

/// A tool's hint goes with the tool when the toolbar fades for the
/// writing, though the pointer rests on it: kui draws a hint at the
/// strip's opacity, not the button's, so it would stay over a toolbar that
/// is gone. The tool still says it.
#[test]
fn a_tools_hint_fades_with_the_toolbar() {
    let (mut app, mut d, dir) = body("hint-fades");
    d.core.set_inspect(true);
    settle(&mut app, &mut d);
    let bold = d.core.nodes().iter().find(|n| n.label.as_deref() == Some("t-bold")).map(|n| (n.key, n.rect)).unwrap();
    d.input(&mut app, InputEvent::CursorMoved(bold.1.center()));
    settle(&mut app, &mut d);
    let hint = |d: &Drive| d.core.nodes().iter().any(|n| n.text.as_deref().is_some_and(|t| t.starts_with("Bold  \u{b7}")));
    assert!(hint(&d), "the hint, the pointer resting on Bold");
    type_in(&mut app, &mut d, "Words going down");
    settle(&mut app, &mut d);
    assert_eq!(tool_opacity(&d, "t-bold"), 0.0);
    assert!(!hint(&d), "gone with the toolbar");
    let said = d.core.access_tree().nodes.iter().find(|n| n.key == bold.0).and_then(|n| n.description.clone());
    assert!(said.as_deref().is_some_and(|s| s.starts_with("Bold")), "{said:?}");
    let _ = std::fs::remove_dir_all(dir);
}


/// The open note's title.
fn open_title(app: &Carnet) -> String {
    app.store.notes[app.current].title.clone()
}

/// The library's notes, by title, in its order.
fn shown_titles(app: &Carnet) -> Vec<String> {
    app.order.iter().map(|&i| app.store.notes[i].title.clone()).collect()
}

/// Note `title` opened, the caret at the start of its body.
fn open_named(app: &mut Carnet, d: &mut Drive, title: &str) {
    let i = app.store.notes.iter().position(|n| n.title == title).unwrap();
    app.open(i);
    app.edit_at(page::first(app.doc()));
    d_frame_twice(app, d);
}

/// Ctrl+Tab opens the next note the library shows — the folder's only,
/// pinned first — and goes round from the last to the first; ⇧ goes
/// back. From the title too, which keeps its words, and where Tab still
/// walks on. ⌥⌘↓ goes down the same list, and stops at its ends. Not
/// from the settings.
#[test]
fn ctrl_tab_goes_round_the_notes_a_folder_shows() {
    let (mut app, mut d, dir) = library("ctrl-tab-folder", &[("Work/Alpha", "1"), ("Work/Bravo", "2"), ("Work/Charlie", "3"), ("Loose", "4")]);
    app.on_settings(Msg::SetSort { to: 1 });
    let charlie = app.store.notes.iter().position(|n| n.title == "Charlie").unwrap();
    app.pin(charlie);
    app.show_folder(Some("Work".into()));
    d_frame_twice(&mut app, &mut d);
    assert_eq!(shown_titles(&app), ["Charlie", "Alpha", "Bravo"]);
    open_named(&mut app, &mut d, "Charlie");
    let in_title = |app: &Carnet, d: &Drive| d.core.focus().and_then(|k| app.fields.get(&k)).copied() == Some(Field::Title);
    app.focus(Field::Title);
    d_frame_twice(&mut app, &mut d);
    assert!(in_title(&app, &d));
    key(&mut app, &mut d, "tab");
    d.frame(&mut app);
    assert!(!in_title(&app, &d), "Tab walks on from the title");
    app.focus(Field::Title);
    d_frame_twice(&mut app, &mut d);
    key_with(&mut app, &mut d, "tab", CTRL);
    d.frame(&mut app);
    assert_eq!(open_title(&app), "Alpha");
    assert_eq!(app.store.notes[charlie].title, "Charlie", "no tab typed in the title");
    assert!(page_focused(&app, &d), "the keyboard in the note come to");
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(open_title(&app), "Bravo");
    // Past the last: the first again, and back.
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(open_title(&app), "Charlie");
    key_with(&mut app, &mut d, "tab", CTRL_SHIFT);
    assert_eq!(open_title(&app), "Bravo");
    key_with(&mut app, &mut d, "tab", CTRL_SHIFT);
    key_with(&mut app, &mut d, "tab", CTRL_SHIFT);
    assert_eq!(open_title(&app), "Charlie");
    // The arrows stop at the first.
    key_with(&mut app, &mut d, "up", CTRL_ALT);
    assert_eq!(open_title(&app), "Charlie");
    key_with(&mut app, &mut d, "down", CTRL_ALT);
    assert_eq!(open_title(&app), "Alpha");
    assert!(app.store.notes.iter().all(|n| !n.markdown().contains('\t')));
    // The settings are no note of the list's.
    key_with(&mut app, &mut d, ",", CTRL);
    d.frame(&mut app);
    assert!(app.settings_open);
    key_with(&mut app, &mut d, "tab", CTRL);
    assert!(app.settings_open, "the settings stay");
    assert_eq!(open_title(&app), "Alpha");
    let _ = std::fs::remove_dir_all(dir);
}

/// Ctrl+Tab from the search goes round what it found, from every folder,
/// and the search keeps what was typed in it.
#[test]
fn ctrl_tab_goes_round_what_the_search_found() {
    let (mut app, mut d, dir) = library(
        "ctrl-tab-search",
        &[("Alpha", "apple pie"), ("Bravo", "banana"), ("Work/Charlie", "apple cider"), ("Delta", "apple tart")],
    );
    app.on_settings(Msg::SetSort { to: 1 });
    open_named(&mut app, &mut d, "Alpha");
    let k = *app.fields.iter().find(|(_, f)| **f == Field::Search).unwrap().0;
    d.focus(&mut app, k);
    d.text(&mut app, "apple");
    d.frame(&mut app);
    assert_eq!(shown_titles(&app), ["Alpha", "Charlie", "Delta"]);
    key_with(&mut app, &mut d, "tab", CTRL);
    d.frame(&mut app);
    assert_eq!(open_title(&app), "Charlie");
    assert_eq!(app.query, "apple");
    assert_eq!(d.core.edit_text(k).as_deref(), Some("apple"), "no tab typed in the search");
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(open_title(&app), "Delta");
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(open_title(&app), "Alpha", "round to the first, past what wasn't found");
    key_with(&mut app, &mut d, "tab", CTRL_SHIFT);
    assert_eq!(open_title(&app), "Delta");
    let _ = std::fs::remove_dir_all(dir);
}

/// Ctrl+Tab wins over Tab's own work: from a table's cell, a list's item
/// or a code block it is the next note, and the note left is as it was —
/// no cell moved to, nothing indented, no tab typed.
#[test]
fn ctrl_tab_from_a_cell_a_list_or_code_changes_only_the_note() {
    let (mut app, mut d, dir) = library(
        "ctrl-tab-blocks",
        &[("Alpha", "| a | b |\n|---|---|\n| c | d |"), ("Bravo", "- one\n- two"), ("Charlie", "```\ncode\n```")],
    );
    app.on_settings(Msg::SetSort { to: 1 });
    let all = |app: &Carnet| -> Vec<String> {
        ["Alpha", "Bravo", "Charlie"].iter().map(|t| app.store.notes.iter().find(|n| n.title == *t).unwrap().markdown()).collect()
    };
    let before = all(&app);
    open_named(&mut app, &mut d, "Alpha");
    let Kind::Table(t) = &blocks(&app)[0].kind else { panic!("{:?}", texts(&app)) };
    let cell = Slot { block: blocks(&app)[0].id, cell: Some((t.rows[1].id, t.cols[0])) };
    app.edit_at(Pos { slot: cell, byte: 1 });
    d_frame_twice(&mut app, &mut d);
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(open_title(&app), "Bravo", "from the cell");
    assert!(page_focused(&app, &d));
    let two = blocks(&app)[1].id;
    app.edit_at(Pos { slot: Slot::block(two), byte: 1 });
    d_frame_twice(&mut app, &mut d);
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(open_title(&app), "Charlie", "from the item");
    assert!(matches!(&blocks(&app)[0].kind, Kind::Raw(r) if r.starts_with("```")), "{:?}", texts(&app));
    let code = blocks(&app)[0].id;
    app.edit_at(Pos { slot: Slot::block(code), byte: 5 });
    d_frame_twice(&mut app, &mut d);
    key_with(&mut app, &mut d, "tab", CTRL_SHIFT);
    assert_eq!(open_title(&app), "Bravo", "back from the code");
    assert!(blocks(&app).iter().all(|b| !matches!(b.kind, Kind::Item { indent: 1.., .. })), "{:?}", texts(&app));
    assert_eq!(all(&app), before);
    let _ = std::fs::remove_dir_all(dir);
}

/// While the library shows what was deleted, Ctrl+Tab goes round that,
/// each to read in turn; the note open behind stays as it was.
#[test]
fn ctrl_tab_in_deleted_goes_round_the_trash() {
    let (mut app, mut d, dir) = library("ctrl-tab-trash", &[("Alpha", "1"), ("Bravo", "2"), ("Charlie", "3")]);
    open_named(&mut app, &mut d, "Alpha");
    for t in ["Bravo", "Charlie"] {
        let i = app.store.notes.iter().position(|n| n.title == t).unwrap();
        app.delete_note_at(i);
    }
    app.on_plain(Msg::ShowDeleted);
    d_frame_twice(&mut app, &mut d);
    assert!(app.deleted);
    assert_eq!(app.trash.len(), 2);
    let reading = |app: &Carnet| app.peek.as_ref().map(|p| p.path.clone());
    let trashed = |app: &Carnet, i: usize| Some(app.trash[i].path.clone());
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(reading(&app), trashed(&app, 0));
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(reading(&app), trashed(&app, 1));
    key_with(&mut app, &mut d, "tab", CTRL);
    assert_eq!(reading(&app), trashed(&app, 0), "round to the first");
    key_with(&mut app, &mut d, "tab", CTRL_SHIFT);
    assert_eq!(reading(&app), trashed(&app, 1));
    assert_eq!(open_title(&app), "Alpha");
    let _ = std::fs::remove_dir_all(dir);
}

/// The finding: a note with a picture at its left edge and a list and
/// code beside it, opened again from another note's card, drew the
/// picture at once and the blocks beside it below it, out of sight, until
/// something else drew the page a second or so later. Beside it from the
/// first frame back, at either edge, the picture still in hand or read
/// from its file again; and opened the first time, beside it within the
/// frames the page asks for.
#[test]
fn the_blocks_beside_a_picture_are_beside_it_from_the_first_frame_back() {
    for (edge, fresh) in [("left", false), ("right", false), ("left", true), ("right", true)] {
        let note = format!(
            "## Sign in\n\n- open it\n\n![shot|{edge}|300](attachments/shot.png)\n\n- the address\n\n```\nhttps://vault.example\n```\n\n\
             - the login\n\n```\nme\n```\n\n- the password\n\n```\nsecret\n```\n\n## After\n\n- one\n- two"
        );
        let (mut app, mut d, dir) = library(&format!("beside-back-{edge}-{fresh}"), &[("Vault", &note), ("Other", "words")]);
        let rgba: Vec<u8> = (0..400 * 300).flat_map(|_| [94u8, 124, 164, 255]).collect();
        let png = crate::images::encode_png(400, 300, &rgba).unwrap();
        std::fs::create_dir_all(dir.join("attachments")).unwrap();
        std::fs::write(dir.join("attachments").join("shot.png"), png).unwrap();
        app.store.index_files();
        app.images.retry_failed();
        let case = format!("{edge}{}", if fresh { ", read again" } else { "" });
        // The picture, and the blocks after it at its side, from its top.
        let beside = |d: &Drive, when: &str| {
            let nodes = d.core.nodes();
            let pic = nodes.iter().find(|n| n.label.as_deref() == Some("pictures")).map(|n| n.rect);
            let pic = pic.unwrap_or_else(|| panic!("{case}, {when}: no picture"));
            assert!(pic.h > 200.0, "{case}, {when}: the picture drawn, {pic:?}");
            for words in ["the address", "https://vault.example", "the login"] {
                let r = nodes.iter().find(|n| n.text.as_deref() == Some(words)).map(|n| n.rect);
                let r = r.unwrap_or_else(|| panic!("{case}, {when}: {words:?} not drawn"));
                let side = if edge == "left" { r.x >= pic.x + pic.w } else { r.x + r.w <= pic.x };
                assert!(side && r.y < pic.y + pic.h, "{case}, {when}: {words:?} at {r:?}, not beside the picture at {pic:?}");
            }
            let first = nodes.iter().find(|n| n.text.as_deref() == Some("the address")).unwrap().rect;
            assert!(first.y < pic.y + 40.0, "{case}, {when}: the first at the picture's top, {first:?} by {pic:?}");
        };
        // Opened the first time: only the frames the app asks for.
        let i = app.store.notes.iter().position(|n| n.title == "Vault").unwrap();
        app.open(i);
        d.frame(&mut app);
        for _ in 0..12 {
            if !d.core.owed().requested {
                break;
            }
            d.advance(0.02);
            d.frame(&mut app);
        }
        beside(&d, "opened");
        open_named(&mut app, &mut d, "Other");
        for _ in 0..4 {
            d.frame(&mut app);
        }
        if fresh {
            // Let go while away: read from its file again on the way back.
            app.images = Default::default();
        }
        // Its card pressed again.
        let i = app.store.notes.iter().position(|n| n.title == "Vault").unwrap();
        app.open(i);
        for when in ["the first frame back", "the second", "the third"] {
            d.advance(0.02);
            d.frame(&mut app);
            beside(&d, when);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

// ------------------------------------------------------------ note windows

/// A note "Plans" with the caret in its body after "draft", and the
/// welcome note behind it.
fn plans(name: &str) -> (Carnet, Drive, std::path::PathBuf) {
    let (mut app, mut d, dir) = fresh(name);
    d.text(&mut app, "Plans");
    d.frame(&mut app);
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "draft");
    app.save_current();
    (app, d, dir)
}

/// The window the main window's frame just opened, as the runner opens
/// it: a core of its own in the main window's session, its frames and
/// events the app's under its window's id.
fn opened_window(app: &mut Carnet, d: &mut Drive) -> (Drive, kui_native::WindowId) {
    d.frame(app);
    let id = d
        .core
        .take_window_commands()
        .into_iter()
        .find_map(|c| match c {
            kui_native::WindowCommand::Open { id, .. } => Some(id),
            _ => None,
        })
        .expect("a window opened");
    let mut core = Core::new_in(d.core.session());
    core.env.window.id = id;
    let mut w = Drive::new(core, 820.0, 740.0).framing();
    w.frame(app);
    w.frame(app);
    (w, id)
}

/// The note window's editor, put by while the main window's is in place.
fn window_caret(app: &Carnet) -> Caret {
    app.windows[0].ed.caret
}

/// Ctrl+Shift+O, or "Open in new window" on a note's card: the note in a
/// window of its own, named for it, with the toolbar and the page and
/// nothing of the library's. The library's keys there are the main
/// window's (a new note opens in it), and the next note is nothing there.
#[test]
fn a_note_opens_in_a_window_of_its_own() {
    let (mut app, mut d, dir) = plans("window-open");
    let plans = app.store.notes[app.current].id;
    key_with(&mut app, &mut d, "o", CTRL_SHIFT);
    assert_eq!(app.windows.iter().map(|w| w.note).collect::<Vec<_>>(), [plans]);
    let (mut w, _) = opened_window(&mut app, &mut d);
    assert_eq!(w.core.window_title(), Some("Plans"));
    assert!(w.key_of("t-bold").is_some() && w.key_of("t-trash").is_some(), "the toolbar");
    assert!(w.key_of("t-sidebar").is_none() && w.key_of("t-zen").is_none(), "nothing of the library's");
    assert!(w.key_of("t-addrow").is_none() && w.key_of("t-out").is_none(), "a paragraph's tools, not a table's or a list's");
    assert!(w.texts_under("body").iter().any(|t| t == "draft"), "{:?}", w.texts_under("body"));
    assert!(!w.core.nodes().iter().any(|n| n.text.as_deref() == Some("Welcome to Carnet")), "no list of notes");
    assert!(w.core.focus().is_some() && w.core.focus() == app.windows[0].ed.page_key, "the page has the keyboard");
    // Renamed in the main window: the window's title follows.
    let title = d.key_of("title").unwrap();
    d.focus(&mut app, title);
    d.key(&mut app, "end", KeyMods::NONE);
    d.text(&mut app, " for May");
    w.frame(&mut app);
    assert_eq!(w.core.window_title(), Some("Plans for May"));
    // The next note and Ctrl+Tab are nothing there; Ctrl+N is a new note
    // in the main window.
    key_with(&mut app, &mut w, "down", KeyMods { alt: true, ..CTRL });
    key_with(&mut app, &mut w, "tab", CTRL);
    w.key(&mut app, "n", CTRL);
    w.frame(&mut app);
    assert_eq!(app.windows[0].ed.current, app.store.notes.iter().position(|n| n.id == plans).unwrap());
    assert_eq!(w.core.window_title(), Some("Plans for May"));
    assert_ne!(app.store.notes[app.current].id, plans, "the main window has the new note");
    assert!(app.store.notes[app.current].title.is_empty());
    assert!(w.warnings().is_empty() && d.warnings().is_empty());

    // From the welcome note's card: a second window, for it.
    d.frame(&mut app);
    let row = d.core.nodes().iter().find(|n| n.text.as_deref() == Some("Welcome to Carnet")).map(|n| n.rect).expect("its card");
    d.button_click(&mut app, row.center().x, row.center().y, kui_native::MouseButton::Secondary);
    d.frame(&mut app);
    let item = d.core.nodes().iter().find(|n| n.text.as_deref() == Some("Open in new window")).map(|n| n.rect.center()).expect("the card's menu");
    d.click(&mut app, item.x, item.y);
    let welcome = app.store.notes.iter().find(|n| n.title == "Welcome to Carnet").unwrap().id;
    assert_eq!(app.windows.iter().map(|w| w.note).collect::<Vec<_>>(), [plans, welcome]);
    let (w2, _) = opened_window(&mut app, &mut d);
    assert_eq!(w2.core.window_title(), Some("Welcome to Carnet"));
    // Asked again: no third window, the one there brought forward.
    app.open_in_window(welcome);
    d.frame(&mut app);
    assert_eq!(app.windows.len(), 2);
    assert!(d.core.take_window_commands().iter().any(|c| matches!(c, kui_native::WindowCommand::Focus(_))));
    let _ = std::fs::remove_dir_all(dir);
}

/// The page's menu opens its note in a window too; one open in both is
/// one note. What is typed in the window is in the main window at once,
/// and back; each window's caret stays with its words through what the
/// other typed, and undo in either takes back the last edit to the note.
#[test]
fn a_note_open_in_two_windows_is_one_note() {
    let (mut app, mut d, dir) = plans("window-both");
    app.page_action("window");
    let (mut w, _) = opened_window(&mut app, &mut d);
    // The window's caret is at the note's start, the main window's after
    // "draft".
    assert_eq!(window_caret(&app).head.byte, 0);
    assert_eq!(app.caret.head.byte, 5);
    type_in(&mut app, &mut w, "First ");
    assert_eq!(texts(&app)[0], "First draft", "in the main window at once");
    assert_eq!(app.caret.head.byte, 11, "its caret carried along with \"draft\"");
    d.frame(&mut app);
    assert!(d.texts_under("body").iter().any(|t| t == "First draft"));
    type_in(&mut app, &mut d, "!");
    assert_eq!(texts(&app)[0], "First draft!");
    assert_eq!(window_caret(&app).head.byte, 6, "the window's caret where it was, before \"draft\"");
    w.frame(&mut app);
    assert!(w.texts_under("body").iter().any(|t| t == "First draft!"));
    // A selection in the window survives a line taken from under it.
    key(&mut app, &mut d, "enter");
    type_in(&mut app, &mut d, "second line");
    key_with(&mut app, &mut w, "end", SHIFT);
    let c = window_caret(&app);
    assert_eq!((c.anchor.byte, c.head.byte), (6, 12));
    key_with(&mut app, &mut d, "a", CTRL);
    key_with(&mut app, &mut d, "a", CTRL);
    key(&mut app, &mut d, "backspace");
    w.frame(&mut app);
    let c = window_caret(&app);
    let doc = &app.store.notes[app.current].doc;
    assert_eq!(page::fix(doc, c), c, "a caret that stands in the note");
    // Undo in the window takes back the main window's last edit.
    key_with(&mut app, &mut w, "z", CTRL);
    assert!(texts(&app).contains(&"second line".to_string()), "{:?}", texts(&app));
    assert!(w.warnings().is_empty() && d.warnings().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// Ctrl+W closes the note window it is pressed in, saving what was typed
/// there, and never the main window; the window can be opened again.
#[test]
fn ctrl_w_closes_the_note_window() {
    let (mut app, mut d, dir) = plans("window-close");
    key_with(&mut app, &mut d, "o", CTRL_SHIFT);
    let (mut w, id) = opened_window(&mut app, &mut d);
    type_in(&mut app, &mut w, "Kept. ");
    key_with(&mut app, &mut w, "w", CTRL);
    assert!(app.windows.is_empty());
    d.frame(&mut app);
    let closed: Vec<_> = d.core.take_window_commands().into_iter().filter(|c| matches!(c, kui_native::WindowCommand::Close(_))).collect();
    assert_eq!(closed, [kui_native::WindowCommand::Close(id)], "that window, not the main one");
    let path = app.store.notes[app.current].path.clone().unwrap();
    assert!(std::fs::read_to_string(path).unwrap().contains("Kept. draft"), "saved as it closed");
    // Its close button, or the OS: the same.
    key_with(&mut app, &mut d, "o", CTRL_SHIFT);
    let (_w, id) = opened_window(&mut app, &mut d);
    assert_eq!(app.windows.len(), 1);
    d.core.window_closed(id);
    for ev in d.core.take_pending_events() {
        app.on_event(ev);
    }
    assert!(app.windows.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// Its note moved to Deleted — from the window's own toolbar, or from the
/// main window — the window closes, and the main window says so with its
/// Undo. A change another app makes to the note draws it again.
#[test]
fn trashing_its_note_closes_the_window() {
    let (mut app, mut d, dir) = plans("window-trash");
    let plans = app.store.notes[app.current].id;
    key_with(&mut app, &mut d, "o", CTRL_SHIFT);
    let (mut w, id) = opened_window(&mut app, &mut d);
    // Changed in another app: the window is drawn again, and shows it.
    let path = app.store.notes[app.current].path.clone().unwrap();
    std::fs::write(&path, "# Plans\n\nredrafted\n").unwrap();
    app.rescan();
    d.frame(&mut app);
    assert!(d.core.take_window_commands().contains(&kui_native::WindowCommand::Redraw(id)));
    w.frame(&mut app);
    assert!(w.texts_under("body").iter().any(|t| t == "redrafted"), "{:?}", w.texts_under("body"));
    let trash = w.key_of("t-trash").unwrap();
    w.click_key(&mut app, trash);
    assert!(app.windows.is_empty());
    assert!(!app.store.notes.iter().any(|n| n.id == plans));
    let notice = app.notice.as_ref().expect("said in the main window");
    assert!(notice.text.contains("Plans") && notice.undo.is_some(), "{}", notice.text);
    d.frame(&mut app);
    assert!(d.core.take_window_commands().contains(&kui_native::WindowCommand::Close(id)));
    // Undo brings it back; opened again and moved to Deleted from the
    // main window, its window closes too.
    app.notice_undo();
    let i = app.store.notes.iter().position(|n| n.title == "Plans").expect("back");
    app.open(i);
    d.frame(&mut app);
    key_with(&mut app, &mut d, "o", CTRL_SHIFT);
    let (_w, _) = opened_window(&mut app, &mut d);
    assert_eq!(app.windows.len(), 1);
    let trash = d.key_of("t-trash").unwrap();
    d.click_key(&mut app, trash);
    assert!(app.windows.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
