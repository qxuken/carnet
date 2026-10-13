//! A copy's second flavour: the Markdown as HTML, so a paste into a mail,
//! a document or a chat keeps its headings, lists, tables and styles.
//! Apps that take only text take the Markdown beside it.

use crate::doc::{Block, CellPicture, Kind, ListStyle, from_markdown, parse_blocks};
use crate::inline;
use std::collections::HashMap;

/// `md` as an HTML fragment. `image_url` turns an image's `src` into
/// something the pasting app can load (a `file:///` URL for a file); it
/// is told when the `src` is an Obsidian embed's (`![[…]]`), a name to
/// look for ([`find_picture`](crate::doc::find_picture)).
pub fn html(md: &str, image_url: impl Fn(&str, bool) -> String) -> String {
    render(&parse_blocks(md), |src, wiki| Picture::Url(image_url(src, wiki)), false)
}

/// Where the viewer's picture is: a URL the page loads as it is (its
/// `src`), or one for the page to fetch itself, kept in `data-picture`
/// with no `src`, so the browser asks for nothing before the page has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picture {
    Url(String),
    Fetch(String),
}

impl Picture {
    /// As the `<img>`'s attribute.
    fn attr(&self) -> String {
        match self {
            Picture::Url(u) => format!("src=\"{}\"", escape(u)),
            Picture::Fetch(p) => format!("data-picture=\"{}\"", escape(p)),
        }
    }
}

/// `md` as the viewer shows it (the PWA, docs/SYNC.md §8): what [`html`]
/// gives, and more of what the desktop draws: real checkboxes, each with
/// `data-task="n"` (its place among the note's checkboxes, from 0) so a
/// tap names it; highlights as `<mark>` (a colour of its own as its
/// `--mark`), the words' colours as `--ink`, for the page's styles to draw
/// in the theme's hues; pictures at an edge, one after another, as a row
/// there (`<div class="pictures left">`, `--n` of them). An image is
/// wherever `image` puts it ([`Picture`]), told as for [`html`] whether
/// its `src` is an Obsidian embed's.
///
/// The note is read whole, as the desktop reads it: its title is its
/// `<h1>`, and front matter is not shown.
pub fn viewer(md: &str, image: impl Fn(&str, bool) -> Picture) -> String {
    let (title, doc) = from_markdown(md);
    let mut out = title.map(|t| format!("<h1>{}</h1>", spans(&t, true, None))).unwrap_or_default();
    let blocks: Vec<Block> = doc.blocks().iter().filter(|b| !matches!(&b.kind, Kind::Raw(r) if r.starts_with("---\n"))).map(|b| (**b).clone()).collect();
    out.push_str(&render(&blocks, image, true));
    out
}

fn render(blocks: &[Block], image: impl Fn(&str, bool) -> Picture, viewer: bool) -> String {
    let spans = |t: &str| spans(t, viewer, None);
    let mut task = 0;
    let mut out = String::new();
    // The open lists, outermost first: their tag, and their indent.
    let mut lists: Vec<(&str, u8)> = Vec::new();
    let close_to = |out: &mut String, lists: &mut Vec<(&str, u8)>, depth: usize| {
        while lists.len() > depth {
            let (tag, _) = lists.pop().unwrap_or(("ul", 0));
            out.push_str(&format!("</{tag}>"));
        }
    };
    // Floated pictures in a row, the viewer's: how many are still to go
    // in the row open.
    let mut row = 0;
    for (i, b) in blocks.iter().enumerate() {
        if let Kind::Item {
            style,
            indent,
            done,
            text,
            ..
        } = &b.kind
        {
            let tag = if *style == ListStyle::Number { "ol" } else { "ul" };
            let depth = *indent as usize + 1;
            close_to(&mut out, &mut lists, depth);
            if lists.len() == depth && lists[depth - 1].0 != tag {
                close_to(&mut out, &mut lists, depth - 1);
            }
            while lists.len() < depth {
                out.push_str(&format!("<{tag}>"));
                lists.push((tag, *indent));
            }
            if *style == ListStyle::Todo && viewer {
                let checked = if *done { " checked" } else { "" };
                out.push_str(&format!("<li class=\"task{}\"><input type=\"checkbox\" data-task=\"{task}\"{checked}><span>{}</span></li>", if *done { " done" } else { "" }, spans(text)));
                task += 1;
                continue;
            } else {
                out.push_str("<li>");
                if *style == ListStyle::Todo {
                    out.push_str(if *done { "\u{2611} " } else { "\u{2610} " });
                }
            }
            out.push_str(&spans(text));
            out.push_str("</li>");
            continue;
        }
        close_to(&mut out, &mut lists, 0);
        match &b.kind {
            // The empty line the editor keeps at the end draws nothing.
            Kind::Text(t) if viewer && t.trim().is_empty() => {}
            // A dropped first letter: the paragraph's class, for the page's
            // styles to drop (`::first-letter`); `own` when the letter has
            // a colour of its own, which the styles then leave it.
            Kind::Text(t) if b.initial && inline::initial_letter(t).is_some() => {
                let own = inline::initial_letter(t).is_some_and(|(f, to)| {
                    let st = inline::styles(t);
                    (f..to).any(|i| !st[i].marker && st[i].ink != 0)
                });
                out.push_str(if own && viewer { "<p class=\"initial own\">" } else { "<p class=\"initial\">" });
                out.push_str(&t.split('\n').map(spans).collect::<Vec<_>>().join("<br>"));
                out.push_str("</p>");
            }
            Kind::Text(t) => {
                out.push_str("<p>");
                out.push_str(&t.split('\n').map(spans).collect::<Vec<_>>().join("<br>"));
                out.push_str("</p>");
            }
            Kind::Quote(t) => {
                out.push_str("<blockquote><p>");
                out.push_str(&t.split('\n').map(spans).collect::<Vec<_>>().join("<br>"));
                out.push_str("</p></blockquote>");
            }
            Kind::Heading { level, text } => {
                let n = (*level).clamp(1, 6);
                out.push_str(&format!("<h{n}>{}</h{n}>", spans(text)));
            }
            Kind::Table(t) => {
                // The viewer's in a box of its own (styles.css `.table`):
                // below any picture at an edge, scrolled when too wide, its
                // lines the page's to draw. A copy's draws its own, for a mail.
                out.push_str(if viewer { "<div class=\"table\"><table>" } else { "<table border=\"1\" cellspacing=\"0\" cellpadding=\"6\">" });
                for (r, row) in t.rows.iter().enumerate() {
                    out.push_str("<tr>");
                    let cell = if t.is_header(r) { "th" } else { "td" };
                    for c in &row.cells {
                        // A cell's pictures among its words, as `image` has them.
                        out.push_str(&format!("<{cell}>{}</{cell}>", self::spans(c, viewer, Some(&image))));
                    }
                    out.push_str("</tr>");
                }
                out.push_str(if viewer { "</table></div>" } else { "</table>" });
            }
            Kind::Divider => out.push_str("<hr>"),
            // The viewer's: pictures at an edge, one after another, in a
            // row there as the desktop has them, the blocks after running
            // beside it (styles.css `.pictures`).
            Kind::Image { src, alt, width, float, wiki } if viewer && *float != crate::doc::Float::None => {
                if row == 0 {
                    row = blocks[i..].iter().take_while(|b| matches!(b.kind, Kind::Image { float, .. } if float != crate::doc::Float::None)).count();
                    let side = if *float == crate::doc::Float::Right { "right" } else { "left" };
                    out.push_str(&format!("<div class=\"pictures {side}\" style=\"--n:{row}\">"));
                }
                let w = width.map(|w| format!(" width=\"{w}\"")).unwrap_or_default();
                out.push_str(&format!("<img {} alt=\"{}\"{w}>", image(src, *wiki).attr(), escape(alt)));
                row -= 1;
                if row == 0 {
                    out.push_str("</div>");
                }
            }
            Kind::Image { src, alt, width, float, wiki } => {
                let mut w = width.map(|w| format!(" width=\"{w}\"")).unwrap_or_default();
                match float {
                    crate::doc::Float::Left => w.push_str(" align=\"left\""),
                    crate::doc::Float::Right => w.push_str(" align=\"right\""),
                    crate::doc::Float::None => {}
                }
                out.push_str(&format!("<p><img {} alt=\"{}\"{w}></p>", image(src, *wiki).attr(), escape(alt)));
            }
            Kind::Raw(t) => {
                // A fence's body, without the fences; its language, as
                // the fence names it, the code's class.
                let body: Vec<&str> = t.lines().collect();
                let fenced = body.len() >= 2 && body[0].trim_start().starts_with(['`', '~']);
                let inner = if fenced { body[1..body.len() - 1].join("\n") } else { t.clone() };
                let lang = body.first().filter(|_| fenced).and_then(|f| f.trim().trim_start_matches(['`', '~']).split_whitespace().next());
                let class = lang.map(|l| format!(" class=\"language-{}\"", escape(l))).unwrap_or_default();
                out.push_str(&format!("<pre><code{class}>{}</code></pre>", escape(&inner)));
            }
            Kind::Item { .. } => {}
        }
    }
    close_to(&mut out, &mut lists, 0);
    out
}

/// Where a picture is put, by its `src` and whether an Obsidian embed's.
type Where<'a> = dyn Fn(&str, bool) -> Picture + 'a;

/// One line's inline styles as tags, its markers left out; with `viewer`,
/// highlights and colours too. With `image`, a table cell's: its pictures
/// among the words, each an `<img>` where `image` puts it, with no width:
/// the cell gives it its room, whatever width or side the note writes.
fn spans(text: &str, viewer: bool, image: Option<&Where<'_>>) -> String {
    let mut out = String::new();
    let _cell = inline::cell_text(image.is_some());
    let pictures = if image.is_some() { inline::cell_pictures(text) } else { Vec::new() };
    let pictures: HashMap<usize, &CellPicture> = pictures.iter().map(|p| (p.at.0, p)).collect();
    // A link's target is a marker run between its text and the next text.
    let runs = inline::runs(text);
    // The link open, one `<a>` round all its words however they are
    // styled (or cut by a bidi control): where its words end.
    let mut link: Option<usize> = None;
    let mut i = 0;
    while i < runs.len() {
        let (a, b, s) = runs[i];
        if link.is_some_and(|end| a >= end) {
            out.push_str("</a>");
            link = None;
        }
        if s.marker {
            i += 1;
            continue;
        }
        if let (true, Some(image), Some(p)) = (s.picture, image, pictures.get(&a)) {
            out.push_str(&format!("<img {} alt=\"{}\">", image(&p.src, p.wiki).attr(), escape(&p.alt)));
            i += 1;
            continue;
        }
        let mut piece = escape(&text[a..b]);
        if s.code {
            piece = format!("<code>{piece}</code>");
        }
        if s.bold {
            piece = format!("<b>{piece}</b>");
        }
        if s.italic {
            piece = format!("<i>{piece}</i>");
        }
        if s.strike {
            piece = format!("<s>{piece}</s>");
        }
        if s.underline {
            piece = format!("<u>{piece}</u>");
        }
        if viewer && s.ink != 0 {
            piece = format!("<span class=\"ink\" style=\"--ink:#{:08x}\">{piece}</span>", s.ink);
        }
        if viewer && s.mark == inline::HIGHLIGHT {
            piece = format!("<mark>{piece}</mark>");
        } else if viewer && s.mark != 0 {
            piece = format!("<mark style=\"--mark:#{:08x}\">{piece}</mark>", s.mark);
        }
        if s.link && link.is_none() {
            // `[text](url)`: the address of the link these words are in,
            // as the page reads it, brackets in it and all. One the page
            // mustn't follow (`javascript:` and the like): its words, with
            // no link.
            if let Some((p, href)) = inline::link_at(text, a).and_then(|(p, u)| Some((p, safe_href(&u)?))) {
                out.push_str(&format!("<a href=\"{}\">", escape(&href)));
                link = Some(p.close.0);
            }
        }
        out.push_str(&piece);
        i += 1;
    }
    if link.is_some() {
        out.push_str("</a>");
    }
    out
}

/// A link's address as a page may follow it: a web or mail address
/// (`http:`, `https:`, `mailto:`), or one with no scheme (a path, `#…`).
/// Any other scheme (`javascript:`, `data:`, `file:`, an app's) is none:
/// a browser would run it, or hand it to whatever claims it. Read as a
/// browser reads it: the ends' spaces and control characters, and tabs
/// and line breaks anywhere, left out, the scheme in any case. A mail
/// link that asks to attach a file (`?attach=…`, `attachment=…`), which
/// some mail apps do with a file of this computer's, is none either.
pub fn safe_href(url: &str) -> Option<String> {
    let url: String = url.trim_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let scheme = url.split_once(':').map(|(s, _)| s).filter(|s| {
        let mut cs = s.chars();
        cs.next().is_some_and(|c| c.is_ascii_alphabetic()) && cs.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    match scheme {
        Some(s) if !["http", "https", "mailto"].iter().any(|k| s.eq_ignore_ascii_case(k)) => None,
        Some(s) if s.eq_ignore_ascii_case("mailto") && attaches(&url) => None,
        _ => Some(url),
    }
}

/// Whether a `mailto:` link names a file to attach: a field of its query
/// whose name, its `%xx` read as letters, has `attach` in it (`attach`,
/// `attachment`, `Attach`, `%61ttach`).
fn attaches(mailto: &str) -> bool {
    let Some((_, query)) = mailto.split_once('?') else { return false };
    query.split(['&', ';']).any(|field| {
        let name = field.split('=').next().unwrap_or("").as_bytes();
        let mut decoded = Vec::with_capacity(name.len());
        let mut i = 0;
        while i < name.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            match (name[i], name.get(i + 1).copied().and_then(hex), name.get(i + 2).copied().and_then(hex)) {
                (b'%', Some(h), Some(l)) => {
                    decoded.push((h * 16 + l) as u8);
                    i += 3;
                }
                (b, _, _) => {
                    decoded.push(b);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&decoded).to_ascii_lowercase().contains("attach")
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_tables_and_styles_come_out_as_html() {
        let md = "Hello **bold** and [docs](http://x)\n\n- [x] done\n- open\n  - nested\n\n1. one\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n![pic](attachments/p.png)";
        let h = html(md, |s, _| format!("file:///C:/n/{s}"));
        assert!(h.contains("<p>Hello <b>bold</b> and <a href=\"http://x\">docs</a></p>"), "{h}");
        assert!(h.contains("<ul><li>\u{2611} done</li><li>open</li><ul><li>nested</li></ul></ul>"), "{h}");
        assert!(h.contains("<ol><li>one</li></ol>"), "{h}");
        assert!(h.contains("<tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr>"), "{h}");
        assert!(h.contains("<img src=\"file:///C:/n/attachments/p.png\" alt=\"pic\">"), "{h}");
    }

    /// Only web, mail and relative links are links: any other scheme, in
    /// any case or broken by a tab, is its words alone.
    #[test]
    fn only_web_mail_and_relative_links_are_links() {
        for bad in ["javascript:alert%281%29", "JaVaScRiPt:alert%281%29", "java\tscript:alert%281%29", " data:text/html,x", "vbscript:x", "file:///etc/passwd", "\u{1}javascript:x", "java\nscript:x"] {
            assert_eq!(safe_href(bad), None, "{bad:?}");
            let h = html(&format!("[x]({bad}) after"), |s, _| s.to_string());
            assert!(!h.contains("href") && (bad.contains('\n') || h.contains("<p>x after</p>")), "{bad:?}: {h}");
            let v = viewer(&format!("# T [y]({bad})\n\n[x]({bad})"), |s, _| Picture::Url(s.into()));
            assert!(!v.contains("href"), "{bad:?}: {v}");
        }
        for good in ["https://x", "HTTP://x/a?b#c", "mailto:a@b", "notes/x.md", "#top", "../a b.md", "//x.org/p", ""] {
            assert_eq!(safe_href(good).as_deref(), Some(good), "{good:?}");
        }
        assert_eq!(safe_href(" https://x\t/a\n "), Some("https://x/a".into()));
        // Mail that would attach a file: no link. Plain mail, with a
        // subject or a body: a link still.
        for bad in ["mailto:a@b?attach=/etc/passwd", "MAILTO:a@b?subject=hi&Attachment=C:\\x", "mailto:a@b?%61ttach=~/.ssh/id_ed25519", "mailto:a@b?body=x;attach=y"] {
            assert_eq!(safe_href(bad), None, "{bad:?}");
        }
        for good in ["mailto:a@b?subject=Attached%20notes&body=see%20attachments", "mailto:a@b?cc=c@d"] {
            assert_eq!(safe_href(good).as_deref(), Some(good), "{good:?}");
        }
        let h = html("[a](https://x) [b](mailto:a@b) [c](notes/x.md) [d](#top)", |s, _| s.to_string());
        for href in ["https://x", "mailto:a@b", "notes/x.md", "#top"] {
            assert!(h.contains(&format!("href=\"{href}\"")), "{href}: {h}");
        }
    }

    /// A link's address is all of it, brackets in pairs too: one that
    /// runs (`javascript:alert(1)`) is its words alone, nothing left over;
    /// one to follow is followed whole, and so are its words in a style.
    #[test]
    fn a_links_address_holds_brackets_in_pairs() {
        let h = html("[x](javascript:alert(1)) after", |s, _| s.to_string());
        assert!(h.contains("<p>x after</p>"), "{h}");
        let h = html("[Mercury](https://x.org/Mercury_(planet)) [a **b**](https://y)", |s, _| s.to_string());
        assert!(h.contains("<a href=\"https://x.org/Mercury_(planet)\">Mercury</a> "), "{h}");
        assert!(h.contains("<a href=\"https://y\">a <b>b</b></a>"), "{h}");
    }

    /// A picture in a table's cell is a picture there, in the reader and
    /// in a copy: where the page fetches it, its alt text escaped; no
    /// width nor side, which the cell decides. A paragraph's is the link
    /// it looks like still.
    #[test]
    fn a_picture_in_a_cell_is_an_img_there() {
        let md = "| img1 | img2 |\n|---|---|\n| ![a \"<b>\"|left|260](../attachments/aerial.jpg) and **words** | ![[photo.png]] |\n\nText ![not](x.png) here";
        let v = viewer(md, |s, wiki| Picture::Fetch(format!("{}{s}", if wiki { "w:" } else { "" })));
        assert!(v.contains("<td><img data-picture=\"../attachments/aerial.jpg\" alt=\"a &quot;&lt;b&gt;&quot;\"> and <b>words</b></td>"), "{v}");
        assert!(v.contains("<td><img data-picture=\"w:photo.png\" alt=\"\"></td>"), "{v}");
        assert!(!v.contains("align") && !v.contains("left") && !v.contains("width"), "{v}");
        assert!(v.contains("<p>Text !<a href=\"x.png\">not</a> here</p>"), "{v}");
        let h = html(md, |s, _| format!("file:///n/{s}"));
        assert!(h.contains("<td><img src=\"file:///n/../attachments/aerial.jpg\" alt=\"a &quot;&lt;b&gt;&quot;\"> and <b>words</b></td>"), "{h}");
        // In code, or after a backslash, no picture.
        let v = viewer("| a | b |\n|---|---|\n| `![](x.png)` | \\![](y.png) |", |s, _| Picture::Fetch(s.into()));
        assert!(!v.contains("<img"), "{v}");
    }

    /// A dropped first letter is the paragraph's class, in a copy and in
    /// the viewer; a letter of its own colour says so, for the viewer.
    #[test]
    fn a_dropped_first_letter_is_a_class_of_its_paragraph() {
        let md = "<span class=\"initial\">O</span>nce upon a time.\n\n<span class=\"initial\" style=\"color: #C0553F\">T</span>he end.\n\nPlain.";
        let h = html(md, |s, _| s.to_string());
        assert!(h.starts_with("<p class=\"initial\">Once upon a time.</p><p class=\"initial\">The end.</p><p>Plain.</p>"), "{h}");
        let v = viewer(md, |s, _| Picture::Url(s.into()));
        assert!(v.contains("<p class=\"initial\">Once"), "{v}");
        assert!(v.contains("<p class=\"initial own\"><span class=\"ink\" style=\"--ink:#c0553fff\">T</span>he end.</p>"), "{v}");
    }

    #[test]
    fn the_viewer_draws_checkboxes_highlights_and_colours() {
        let md = "- [ ] milk\n- [x] bread\n  - [ ] rye\n\nSee ==this== and <mark style=\"background:#ffd0d0\">that</mark>, <span style=\"color:#c03030\">red</span>.";
        let h = viewer(md, |s, _| Picture::Url(format!("blob:{s}")));
        assert!(h.contains("<li class=\"task\"><input type=\"checkbox\" data-task=\"0\"><span>milk</span></li>"), "{h}");
        assert!(h.contains("<li class=\"task done\"><input type=\"checkbox\" data-task=\"1\" checked><span>bread</span></li>"), "{h}");
        assert!(h.contains("data-task=\"2\"><span>rye"), "{h}");
        assert!(h.contains("<mark>this</mark>"), "{h}");
        assert!(h.contains("<mark style=\"--mark:#"), "{h}");
        assert!(h.contains("<span class=\"ink\" style=\"--ink:#c03030ff\">red</span>"), "{h}");
        // Front matter isn't drawn, nor counted: box 0 is the first real
        // one, as toggle_task counts.
        let fm = "---\ntags: [x]\n- [ ] in front matter\n---\n# Title\n\n- [ ] real\n";
        let v = viewer(fm, |s, _| Picture::Url(s.into()));
        assert!(v.starts_with("<h1>Title</h1>") && v.contains("data-task=\"0\"><span>real") && !v.contains("front matter"), "{v}");
        assert_eq!(crate::doc::toggle_task(fm, 0).unwrap(), fm.replacen("- [ ] real", "- [x] real", 1));
        // Pictures at an edge, one after another: a row at that edge.
        let row = viewer("![a|left](a.png)\n![b|right|300](b.png)\n\n---\n\nWords.\n\n![c|right](c.png)\n", |s, _| Picture::Url(s.into()));
        assert!(
            row.contains("<div class=\"pictures left\" style=\"--n:2\"><img src=\"a.png\" alt=\"a\"><img src=\"b.png\" alt=\"b\" width=\"300\"></div><hr><p>Words.</p><div class=\"pictures right\" style=\"--n:1\"><img src=\"c.png\" alt=\"c\"></div>"),
            "{row}"
        );
        assert!(html("![a|left](a.png)", |s, _| s.to_string()).contains("<p><img src=\"a.png\" alt=\"a\" align=\"left\"></p>"), "the copy keeps align");
        // A picture the page fetches itself: no `src` for the browser to
        // ask for before it has it.
        let fetched = viewer("![a|left](a.png)\n\n![b](b.png)\n", |s, _| Picture::Fetch(format!("x/{s}")));
        assert!(fetched.contains("<img data-picture=\"x/a.png\" alt=\"a\">") && fetched.contains("<p><img data-picture=\"x/b.png\" alt=\"b\"></p>"), "{fetched}");
        assert!(!fetched.contains("src="), "{fetched}");
        // The copy for mail stays as it was.
        assert!(html(md, |s, _| s.to_string()).contains("\u{2610} milk"));
    }

    /// The viewer's table is in a box of its own, the page's styles to
    /// frame and to put below pictures at an edge; its divider an `<hr>`
    /// they draw as the desktop's dots. A copy's table draws its own lines.
    #[test]
    fn the_viewers_table_is_boxed_and_its_divider_plain() {
        let md = "# T\n\n![|left|330](a.png)\n\n## Plan\n\n| Day | Miles |\n|---|---|\n| Fri | 30 |\n\n---\n\n> A quote.\n";
        let v = viewer(md, |s, _| Picture::Url(s.into()));
        assert!(
            v.contains("<div class=\"pictures left\" style=\"--n:1\"><img src=\"a.png\" alt=\"\" width=\"330\"></div><h2>Plan</h2><div class=\"table\"><table><tr><th>Day</th><th>Miles</th></tr><tr><td>Fri</td><td>30</td></tr></table></div><hr><blockquote>"),
            "{v}"
        );
        let h = html(md, |s, _| s.to_string());
        assert!(h.contains("<table border=\"1\" cellspacing=\"0\" cellpadding=\"6\"><tr><th>Day</th>") && !h.contains("class=\"table\""), "{h}");
    }

    /// Autolinks are links in the reader, only to the web or a mail (an
    /// address alone a mail's); a link's words come out without bidi
    /// controls, so none turns round the words after it (the finding:
    /// `abc.dne exe.txt`).
    #[test]
    fn autolinks_and_bidi_controls_in_a_links_words() {
        let v = viewer("<https://example.com/x> and <mailto:a@b.c> or <d@e.f>, not <javascript:x>", |s, _| Picture::Url(s.into()));
        assert!(v.contains("<a href=\"https://example.com/x\">https://example.com/x</a>"), "{v}");
        assert!(v.contains("<a href=\"mailto:a@b.c\">mailto:a@b.c</a>"), "{v}");
        assert!(v.contains("<a href=\"mailto:d@e.f\">d@e.f</a>"), "{v}");
        assert!(v.contains("not &lt;javascript:x&gt;"), "{v}");
        let v = viewer("Bidi [abc\u{202E}txt.exe](https://example.com/y) end.", |s, _| Picture::Url(s.into()));
        let words: String = v.split('<').map(|p| p.split_once('>').map_or(p, |(_, w)| w)).collect();
        assert_eq!(words, "Bidi abctxt.exe end.", "{v}");
        assert!(v.contains("<a href=\"https://example.com/y\">abctxt.exe</a> end."), "{v}");
        assert!(!v.contains('\u{202E}'), "{v}");
    }

    /// The finding: a link with a bidi control or partly styled words
    /// came out as an `<a>` per run, side by side to the same address.
    /// One link is one `<a>`, round all its styled words; two links side
    /// by side are two.
    #[test]
    fn one_link_is_one_a() {
        let h = |md: &str| viewer(md, |s, _| Picture::Url(s.into()));
        let v = h("See [the **bold** and *slanted* ==words==](https://x.test/a) now.");
        assert_eq!(v.matches("<a ").count(), 1, "{v}");
        assert!(v.contains("<p>See <a href=\"https://x.test/a\">the <b>bold</b> and <i>slanted</i> <mark>words</mark></a> now.</p>"), "{v}");
        let v = h("[a\u{2066}b](https://x.test/a)[c](https://x.test/c) and [**all bold**](#top)");
        assert!(v.contains("<p><a href=\"https://x.test/a\">ab</a><a href=\"https://x.test/c\">c</a> and <a href=\"#top\"><b>all bold</b></a></p>"), "{v}");
        // One the page mustn't follow: its words, styled, no link.
        let v = h("[a **b**](javascript:x) end");
        assert!(v.contains("<p>a <b>b</b> end</p>"), "{v}");
        // In a copy's HTML too.
        let c = html("[x *y*](https://x.test)", |s, _| s.to_string());
        assert!(c.contains("<a href=\"https://x.test\">x <i>y</i></a>"), "{c}");
    }

    /// Underlined words come out as `<u>` in a copy and in the reader,
    /// beside the other styles and in a link's words; `<ins>` as `<u>`.
    #[test]
    fn underline_comes_out_as_u() {
        let md = "Some <u>under</u> and **<u>both</u>**, [<u>linked</u>](http://x), <ins>theirs</ins>.";
        let h = html(md, |s, _| s.to_string());
        assert_eq!(
            h,
            "<p>Some <u>under</u> and <u><b>both</b></u>, <a href=\"http://x\"><u>linked</u></a>, <u>theirs</u>.</p>"
        );
        let v = viewer("# T\n\n<span style=\"color: #C0553F\"><u>red</u></span>\n", |s, _| Picture::Url(s.to_string()));
        assert!(v.contains("<span class=\"ink\" style=\"--ink:#c0553fff\"><u>red</u></span>"), "{v}");
        // A tag left open is words, escaped.
        assert_eq!(html("a <u>b", |s, _| s.to_string()), "<p>a &lt;u&gt;b</p>");
    }
}
