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
        .find(|n| n.text.as_deref() == Some("Move to trash"))
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
    d.frame(&mut app);
    let at = Vec2::new(600.0, 300.0);
    let paths = vec![a.display().to_string(), txt.display().to_string()];
    d.input(&mut app, InputEvent::DragFiles { paths: paths.clone(), at });
    d.input(&mut app, InputEvent::DropFiles { paths, at });
    d.frame(&mut app);
    let srcs = image_blocks(&app);
    assert_eq!(srcs, ["attachments/Holiday-photo.png"]);
    assert!(dir.join("attachments").join("Holiday-photo.png").is_file());
    assert!(a.exists(), "copied, not moved");
    assert!(app.notice.as_ref().is_some_and(|n| n.text.contains("isn't a picture")));
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
    assert_eq!(app.settings.colors.first(), Some(&0xc0553fff), "and the colour is a recent one");
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
    assert!(names(&app).contains("Move folder to trash"), "{}", names(&app));
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
        let (page, fg) = if dark { (Color::hex(0x1f1b18ff), Color::hex(0xefe6dcff)) } else { (Color::hex(0xfcfaf7ff), Color::hex(0x2f2a27ff)) };
        for ink in &HUES {
            for mark in &HUES {
                let c = crate::design::tint::ink((ink.ink << 8) | 0xff, dark);
                let w = wash(mark.wash, dark, page);
                assert!(crate::design::contrast(c, w) >= 3.5 || !dark, "{} on {}: readable in the dark", ink.name, mark.name);
                assert_eq!(super::page_view::words_ink(Some(c), fg, Some(w), page), c, "{} on {} ({})", ink.name, mark.name, if dark { "dark" } else { "light" });
            }
        }
    }
    // Black words on a dark page still give way; plain words on a wash
    // read.
    let page = Color::hex(0x1f1b18ff);
    assert_ne!(super::page_view::words_ink(Some(Color::hex(0x000000ff)), Color::hex(0xefe6dcff), None, page), Color::hex(0x000000ff));
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

/// Whether a drawn node reads `s`.
fn drawn(d: &mut Drive, app: &mut Carnet, s: &str) -> bool {
    d.core.set_inspect(true);
    d.frame(app);
    d.core.nodes().iter().any(|n| n.text.as_deref() == Some(s))
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
    // One field for a code, whichever kind; the words are asked for apart.
    assert!(drawn(&mut d, &mut app, "Use recovery words"));
    assert!(!drawn(&mut d, &mut app, "Pairing code"), "no kinds of code to choose between");
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
    app.on_plain(Msg::SyncHow { to: 1 });
    d.frame(&mut app);
    assert!(drawn(&mut d, &mut app, "Recover"));
    assert!(drawn(&mut d, &mut app, "Use a code"));
    // The third way: an email and password, the password as dots.
    assert!(drawn(&mut d, &mut app, "Use email and password"));
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
    app.sync_how = 2;
    app.sync_password = "hunter22".into();
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
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new() }, listener, None, host, async {
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
    a.pw_edit = Some(("Ann@Example.com".into(), "correct horse battery".into()));
    a.on_plain(Msg::SyncPasswordSave);
    until("set", &mut a, &mut da, &|x| x.notice.as_ref().is_some_and(|n| n.text.starts_with("Password set")));
    until("listed", &mut a, &mut da, &|x| x.sync.password == Some((true, Some("ann@example.com".into()))));

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
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new() }, listener, None, host, async {
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
    let served = rt.spawn(server::serve_hosted(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new() }, listener, None, server::Host::default(), async {
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
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 4, web: None, trusted: Vec::new() }, listener, async {
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
    // B's own welcome note, untouched, went to its trash and was never
    // sent. Each has the same notes on disk, and only those.
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
    until("A lists three devices", &mut [(&mut a, &mut da), (&mut b, &mut db), (&mut c, &mut dc)], &|x| {
        x[0].0.sync.devices.as_ref().is_some_and(|l| l.len() == 3 && l.iter().all(|d| d.name.is_some()))
    });
    let list = a.sync.devices.clone().unwrap();
    let mut names: Vec<(String, bool)> = list.iter().map(|d| (d.name.clone().unwrap(), d.you)).collect();
    names.sort();
    assert_eq!(names, [("Desk".into(), true), ("Laptop".into(), false), ("Tablet".into(), false)]);
    // Each with its id, short, after what it is: two of one name told apart.
    let short = |id: &[u8]| id.iter().take(3).map(|b| format!("{b:02x}")).collect::<String>();
    let me = list.iter().find(|d| d.you).unwrap();
    assert!(drawn(&mut da, &mut a, "Laptop") && drawn(&mut da, &mut a, &format!("This device \u{b7} {}", short(&me.id))));
    let at = list.iter().position(|d| d.name.as_deref() == Some("Laptop")).unwrap() as u64;
    a.on_plain(Msg::SyncRemove { at });
    assert!(b.sync.is_on() && a.sync_remove_armed.is_some(), "asked once: not yet");
    assert!(drawn(&mut da, &mut a, "Remove for real"));
    a.on_plain(Msg::SyncRemove { at });
    until("the laptop is out", &mut [(&mut a, &mut da), (&mut b, &mut db), (&mut c, &mut dc)], &|x| {
        !x[1].0.sync.is_on() && x[0].0.sync.devices.as_ref().is_some_and(|l| l.len() == 2)
    });
    assert!(b.sync.error.as_deref().is_some_and(|e| e.contains("removed")), "{:?}", b.sync.error);
    // Off again, its Settings show an empty code field, not the old code.
    b.settings_open = true;
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

    // Turn off asks twice, and anything between disarms it.
    a.on_plain(Msg::SyncOff);
    assert!(a.sync.is_on() && a.sync_off_armed);
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
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new() }, listener, async {
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
    assert!(a.sync_account_armed.is_some() && drawn(&mut da, &mut a, "Remove for real"), "asked once: not yet");
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
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new() }, listener, async {
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
    a.sync_reset_fields = true;
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
    assert!(said == "Moved 1 picture and 1 other file to the attachments folder, links and all", "{said}");
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
    assert!(!app.folder_is_bare(&f), "the menu says Move folder to trash");
    app.folder_action(&f, "delete");
    assert!(app.error.is_none(), "{:?}", app.error);
    assert!(!dir.join("Old").exists());
    assert!(app.notice.as_ref().is_some_and(|n| n.text.starts_with("Moved \u{201c}Old\u{201d} to the trash") && n.undo.is_some()));
    assert!(app.trash.iter().any(|t| t.is_folder && t.name == "Old"));
    // Nothing in it at all: just removed.
    let g = app.store.create_folder("", "Bare").unwrap();
    assert!(app.folder_is_bare(&g), "the menu says Delete folder");
    app.folder_action(&g, "delete");
    assert!(app.error.is_none() && !dir.join("Bare").exists());
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
    // Each way of drawing a divider, and a dropped first letter, draw.
    app.page_action("initial");
    for to in 0..crate::settings::Divider::ALL.len() as u64 {
        app.on_settings(Msg::SetDivider { to });
        d_frame_twice(&mut app, &mut d);
        assert!(row(&app, 1).h > 10.0);
    }
    assert_eq!(app.settings.divider, crate::settings::Divider::Wave);
    assert!(blocks(&app)[2].initial);
    let w = d.warnings();
    assert!(w.is_empty(), "{w:?}");
    let _ = std::fs::remove_dir_all(dir);
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
    assert_eq!(said, "Moved a folder and 2 notes to the trash");
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
    assert!(menu.contains(&"Move 2 notes to".to_string()) && menu.contains(&"Move 2 notes to the trash".to_string()), "{menu:?}");
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
    assert!(!enabled(&mut d, "Reset"), "nothing to reset yet");
    app.on_plain(Msg::SetSize { by: 3 });
    app.on_plain(Msg::SetPalette { to: 2 });
    app.on_plain(Msg::SetFocus { to: 1 });
    app.on_plain(Msg::SetSort { to: 1 });
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
    assert!(!app.reset_armed && !enabled(&mut d, "Reset"));
    assert!(app.notice.as_ref().is_some_and(|n| n.text.starts_with("Appearance and writing are back")));
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
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: 200, web: None, trusted: Vec::new() }, listener, async {
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

/// A picture that's the user's own file, beside the note: drawn faint with
/// a mark, and selected, a bar that says so. Moved into Carnet, it's in
/// the attachments folder and the note shows it there; the notice's Undo
/// (Ctrl+Z) puts the file back, and an undo of the typing before never
/// brings back a path that leads nowhere. Copied, only this note shows
/// the copy, and Ctrl+Z takes the edit back.
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
