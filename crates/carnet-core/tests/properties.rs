//! Property tests: notes made of random pieces — words, scripts, emoji,
//! every mark Carnet reads, half of a mark, a table, a fence — and random
//! edits of them. proptest shrinks a failure to the smallest note that
//! shows it; one found is kept in `proptest-regressions/` and tried first
//! from then on.
//!
//! What must hold:
//! - nothing panics, on any text at all: styling, cutting, typing,
//!   comparing;
//! - on notes as people write them (marks in pairs, lines ending in
//!   `\n` or `\r\n`), the Markdown round trip settles after one pass,
//!   and styling a stretch of words changes its marks, never its words;
//! - comparing two versions marks nothing when they are the same, and,
//!   for plain words, keeps every word of the newer one.
//!
//! The `wild_` tests ask the same further: the round trip, joining pairs
//! and styling on any text at all — a lone `*`, a lone `\r`, a line of
//! one `\` — and on lines whose pairs sit next to and inside each other.
//!
//! A case that never ends fails the run, the case printed (`watch`).

use carnet_core::compare;
use carnet_core::doc;
use carnet_core::inline::{self, Kind, Visible};
use proptest::prelude::*;
use proptest::sample::{Index, select};
use std::io::Write;
use std::sync::{Mutex, Once};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// Pieces of a line: what text inside one block is made of.
const INLINE: &[&str] = &[
    "word", "Ünï", "é", "ё", "名前", "👩\u{200d}👩\u{200d}👧", "e\u{301}", " ", "  ", "\t", "**", "*", "_", "~~", "==",
    "`", "[", "](", "](https://example.com)", ")", "<mark>", "</mark>", "<mark style=\"background: #D8EAC0CC\">",
    "<span style=\"color: #C0553F\">", "<span style=\"color: #C0553F; background: #F8EBA6CC\">", "</span>", "<u>", "</u>",
    "<ins>", "</ins>", "\\", "|",
    "#", ">", "1.", "-", "!", "[x]",
];

/// Pieces that make blocks.
const BLOCK: &[&str] = &[
    "\n", "\n\n", "\r\n", "- ", "* ", "1. ", "- [ ] ", "- [x] ", "    - ", "# ", "## ", "> ", "---\n",
    "```\ncode\n```\n", "| a | b |\n|---|---|\n| 1 | 2 |\n", "![](attachments/a.png)\n", "![|right|300](a.png)\n",
    "| ![|left|260](a.png) | b |\n|---|---|\n| ![[c.png\\|9]] x | ![a](<b c.png>) |\n",
];

fn line() -> impl Strategy<Value = String> {
    prop::collection::vec(select(INLINE), 0..24).prop_map(|v| v.concat())
}

/// Lines as Carnet writes them: every mark in a pair.
const BALANCED: &[&str] = &[
    "word", "Ünï", "名前", "👩\u{200d}👩\u{200d}👧", " ", "**bold**", "*it*", "~~gone~~", "==lit==", "`code`",
    "[link](https://example.com)", "<mark style=\"background: #D8EAC0CC\">wash</mark>",
    "<span style=\"color: #C0553F\">red</span>", "**a *b* c**", "<u>under</u>", "<u>**both**</u>",
];

fn balanced_line() -> impl Strategy<Value = String> {
    // Apart: two pairs touching (`==a====b==`) are one to Carnet, which
    // writes them joined.
    prop::collection::vec(select(BALANCED), 0..16).prop_map(|v| v.join(" "))
}

fn note() -> impl Strategy<Value = String> {
    let piece = prop_oneof![3 => select(INLINE), 1 => select(BLOCK)];
    prop_oneof![
        8 => prop::collection::vec(piece, 0..48).prop_map(|v| v.concat()),
        // And now and then anything at all.
        1 => any::<String>(),
    ]
}

/// A note as people write them: lines end in `\n` or `\r\n`, and no
/// line is a lone `\` (pieces of marks are still everywhere).
fn written_note() -> impl Strategy<Value = String> {
    note().prop_map(|s| {
        let s = s.replace("\r\n", "\n").replace('\r', "");
        s.lines().filter(|l| l.trim() != "\\").collect::<Vec<_>>().join("\n")
    })
}

/// A piece put in or a stretch taken out, somewhere.
#[derive(Clone, Debug)]
enum Change {
    Put(Index, &'static str),
    Cut(Index, usize),
}

fn changes() -> impl Strategy<Value = Vec<Change>> {
    let one = prop_oneof![
        (any::<Index>(), select([INLINE, BLOCK].concat())).prop_map(|(i, s)| Change::Put(i, s)),
        (any::<Index>(), 1..12usize).prop_map(|(i, n)| Change::Cut(i, n)),
    ];
    prop::collection::vec(one, 1..6)
}

fn boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn apply(s: &str, changes: &[Change]) -> String {
    let mut s = s.to_string();
    for c in changes {
        match c {
            Change::Put(i, p) => {
                let at = boundary(&s, i.index(s.len() + 1));
                s.insert_str(at, p);
            }
            Change::Cut(i, n) => {
                let a = boundary(&s, i.index(s.len() + 1));
                let b = boundary(&s, a + n);
                s.replace_range(a..b, "");
            }
        }
    }
    s
}

fn round_trip(s: &str) -> String {
    let (title, d) = doc::from_markdown(s);
    doc::to_markdown(title.as_deref(), &d)
}

/// Where the page would put a caret for word place `i`: between letters,
/// inside the marks of the letter before, as the editor does.
fn caret(text: &str, i: Index) -> usize {
    let vis = Visible::of(text);
    let words = vis.text(text);
    let d = boundary(&words, i.index(words.len() + 1));
    vis.at(d)
}

/// The words of a marked text: the struck ones and every tag taken out.
fn unmarked(text: &str) -> String {
    let mut s = text.to_string();
    // Struck words: `~~<span …>…</span>~~`.
    while let Some(a) = s.find("~~<span") {
        let Some(b) = s[a..].find("</span>~~") else { break };
        s.replace_range(a..a + b + "</span>~~".len(), " ");
    }
    while let Some(a) = s.find("<mark") {
        let b = s[a..].find('>').map_or(s.len(), |b| a + b + 1);
        s.replace_range(a..b, "");
    }
    s.replace("</mark>", "")
}

fn words(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

/// `n` cases, or as many as `PROPTEST_CASES` asks for: a deep run is
/// `PROPTEST_CASES=10000 cargo test --release`.
fn cases(n: u32) -> ProptestConfig {
    let n = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(n);
    ProptestConfig::with_cases(n)
}

/// The cases running now, by thread: when each began and what it is.
static RUNNING: Mutex<Vec<(ThreadId, Instant, String)>> = Mutex::new(Vec::new());

/// A case that runs too long fails instead of spinning: each case says
/// what it is working on, and a watchdog aborts the run, printing it,
/// once one has run `PROPTEST_HANG_SECS` seconds (60 if unset). proptest
/// cannot time a case out without forking, and a hang never shrinks.
struct Watch;

fn watch(what: impl FnOnce() -> String) -> Watch {
    static DOG: Once = Once::new();
    DOG.call_once(|| {
        let secs = std::env::var("PROPTEST_HANG_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
        let limit = Duration::from_secs(secs);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
                if let Some((_, _, what)) = running.iter().find(|(_, at, _)| at.elapsed() > limit) {
                    // Straight to stderr: the test harness keeps what
                    // `eprintln!` says, and an abort loses it.
                    let _ = writeln!(std::io::stderr(), "a case has run over {secs} s, so the run stops here. The case:\n{what}");
                    std::process::abort();
                }
            }
        });
    });
    let me = std::thread::current().id();
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    running.retain(|(id, _, _)| *id != me);
    running.push((me, Instant::now(), what()));
    Watch
}

impl Drop for Watch {
    fn drop(&mut self) {
        let me = std::thread::current().id();
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _, _)| *id != me);
    }
}

proptest! {
    #![proptest_config(cases(512))]

    /// What Carnet writes it reads back the same: written once, a note
    /// written again is byte for byte what it was.
    #[test]
    fn the_markdown_round_trip_settles(s in written_note()) {
        let _watch = watch(|| format!("s: {s:?}"));
        let once = round_trip(&s);
        prop_assert_eq!(round_trip(&once), once);
    }

    #[test]
    fn wild_the_markdown_round_trip_settles(s in note()) {
        let _watch = watch(|| format!("s: {s:?}"));
        let once = round_trip(&s);
        prop_assert_eq!(round_trip(&once), once);
    }

    /// Any line at all, stray marks and all: styling it never panics.
    #[test]
    fn styling_never_panics(
        text in line(),
        a in any::<Index>(),
        b in any::<Index>(),
        which in 0..7usize,
        color in select(&[inline::HIGHLIGHT, compare::ADDED, compare::REMOVED, 0][..]),
    ) {
        let (a, b) = (caret(&text, a), caret(&text, b));
        let (a, b) = (a.min(b), a.max(b));
        let _watch = watch(|| format!("text: {text:?}, a: {a}, b: {b}, which: {which}, color: {color:#x}"));
        let _ = match which {
            0 => inline::emphasis(&text, a, b, Kind::Bold),
            1 => inline::emphasis(&text, a, b, Kind::Italic),
            2 => inline::emphasis(&text, a, b, Kind::Strike),
            3 => inline::emphasis(&text, a, b, Kind::Code),
            4 => inline::tint(&text, a, b, true, color),
            5 => inline::tint(&text, a, b, false, color),
            _ => inline::emphasis(&text, a, b, Kind::Underline),
        };
    }

    /// A line as Carnet writes it: styling a stretch changes its marks,
    /// never its words, pairs next to and inside each other (`~~a~~
    /// `b` **c**`, `**a *b* c**`) and all.
    #[test]
    fn wild_styling_keeps_the_words(
        text in balanced_line(),
        a in any::<Index>(),
        b in any::<Index>(),
        which in 0..7usize,
        color in select(&[inline::HIGHLIGHT, compare::ADDED, compare::REMOVED, 0][..]),
    ) {
        let (a, b) = (caret(&text, a), caret(&text, b));
        let (a, b) = (a.min(b), a.max(b));
        let _watch = watch(|| format!("text: {text:?}, a: {a}, b: {b}, which: {which}, color: {color:#x}"));
        let styled = match which {
            0 => inline::emphasis(&text, a, b, Kind::Bold),
            1 => inline::emphasis(&text, a, b, Kind::Italic),
            2 => inline::emphasis(&text, a, b, Kind::Strike),
            3 => inline::emphasis(&text, a, b, Kind::Code),
            4 => inline::tint(&text, a, b, true, color),
            5 => inline::tint(&text, a, b, false, color),
            _ => inline::emphasis(&text, a, b, Kind::Underline),
        };
        if let Some((out, _, _)) = styled {
            prop_assert_eq!(inline::plain(&out), inline::plain(&text));
        }
    }

    /// A line as Carnet writes it: styling a stretch that holds words
    /// always styles it; none needs marks that cannot say it.
    #[test]
    fn styling_a_written_line_always_styles(
        text in balanced_line(),
        a in any::<Index>(),
        b in any::<Index>(),
        which in 0..7usize,
        color in select(&[inline::HIGHLIGHT, compare::ADDED, compare::REMOVED, 0][..]),
    ) {
        let (a, b) = (caret(&text, a), caret(&text, b));
        let (a, b) = (a.min(b), a.max(b));
        let _watch = watch(|| format!("text: {text:?}, a: {a}, b: {b}, which: {which}, color: {color:#x}"));
        let styled = match which {
            0 => inline::emphasis(&text, a, b, Kind::Bold),
            1 => inline::emphasis(&text, a, b, Kind::Italic),
            2 => inline::emphasis(&text, a, b, Kind::Strike),
            3 => inline::emphasis(&text, a, b, Kind::Code),
            4 => inline::tint(&text, a, b, true, color),
            5 => inline::tint(&text, a, b, false, color),
            _ => inline::emphasis(&text, a, b, Kind::Underline),
        };
        let vis = Visible::of(&text);
        let words = &vis.text(&text)[vis.drawn(a)..vis.drawn(b)];
        if !words.trim().is_empty() {
            prop_assert!(styled.is_some(), "{:?} left alone", words);
        }
    }

    /// Any line at all, stray marks and all: styling a stretch of it
    /// never changes its words either. A stray that a new mark would take
    /// for one of a pair is escaped (`\*`); where no marks can say it
    /// (bold on the `*` of `a*b`), the line is left as it is.
    #[test]
    fn wild_styling_any_line_keeps_the_words(
        text in line(),
        a in any::<Index>(),
        b in any::<Index>(),
        which in 0..7usize,
        color in select(&[inline::HIGHLIGHT, compare::ADDED, compare::REMOVED, 0][..]),
    ) {
        let (a, b) = (caret(&text, a), caret(&text, b));
        let (a, b) = (a.min(b), a.max(b));
        let _watch = watch(|| format!("text: {text:?}, a: {a}, b: {b}, which: {which}, color: {color:#x}"));
        let styled = match which {
            0 => inline::emphasis(&text, a, b, Kind::Bold),
            1 => inline::emphasis(&text, a, b, Kind::Italic),
            2 => inline::emphasis(&text, a, b, Kind::Strike),
            3 => inline::emphasis(&text, a, b, Kind::Code),
            4 => inline::tint(&text, a, b, true, color),
            5 => inline::tint(&text, a, b, false, color),
            _ => inline::emphasis(&text, a, b, Kind::Underline),
        };
        if let Some((out, _, _)) = styled {
            prop_assert_eq!(inline::plain(&out), inline::plain(&text));
        }
    }

    #[test]
    fn cutting_and_typing_never_panic(text in line(), a in any::<Index>(), b in any::<Index>(), typed in line()) {
        let (a, b) = (caret(&text, a), caret(&text, b));
        let (a, b) = (a.min(b), a.max(b));
        let _watch = watch(|| format!("text: {text:?}, a: {a}, b: {b}, typed: {typed:?}"));
        let _ = inline::cut(&text, a, b);
        let _ = inline::delete(&text, a, b);
        let _ = inline::split(&text, a);
        let _ = inline::slice(&text, a, b);
        let _ = inline::insert_styled(&text, a, &typed, "**", "**");
        let _ = inline::merge(&text);
    }

    /// Pairs left touching are joined, and the words stay.
    #[test]
    fn joining_pairs_keeps_the_words(text in balanced_line()) {
        let _watch = watch(|| format!("text: {text:?}"));
        prop_assert_eq!(inline::plain(&inline::merge(&text)), inline::plain(&text));
    }

    #[test]
    fn wild_joining_pairs_keeps_the_words(text in line()) {
        let _watch = watch(|| format!("text: {text:?}"));
        prop_assert_eq!(inline::plain(&inline::merge(&text)), inline::plain(&text));
    }

    #[test]
    fn comparing_never_panics_and_the_same_marks_nothing(old in note(), edits in changes()) {
        let _watch = watch(|| format!("old: {old:?}, edits: {edits:?}"));
        let new = apply(&old, &edits);
        let _ = compare::marked(&old, &new);
        let (_, same) = compare::marked(&new, &new);
        let (_, plain) = doc::from_markdown(&new);
        prop_assert_eq!(same.len(), plain.len());
        for (m, p) in same.blocks().iter().zip(plain.blocks()) {
            prop_assert_eq!(m.text_ref(), p.text_ref());
        }
    }

    /// For plain words the marks are only marks: take them out and the
    /// struck words, and the newer version's words are all there, in
    /// order.
    #[test]
    fn a_compared_paragraph_keeps_the_new_words(
        old in prop::collection::vec(select(&["the", "quick", "brown", "fox", "Ünï", "名前", "👩\u{200d}👩\u{200d}👧"][..]), 1..12),
        new in prop::collection::vec(select(&["the", "slow", "brown", "fox", "jumps", "é", "名前"][..]), 1..12),
    ) {
        let (old, new) = (old.join(" "), new.join(" "));
        let _watch = watch(|| format!("old: {old:?}, new: {new:?}"));
        let (_, d) = compare::marked(&format!("# N\n\n{old}\n"), &format!("# N\n\n{new}\n"));
        let text = d.blocks()[0].text_ref().unwrap_or("");
        let left = unmarked(text);
        prop_assert_eq!(words(&left), words(&new));
    }
}
