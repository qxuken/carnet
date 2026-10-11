//! A copy's second flavour: the Markdown as HTML, so a paste into a mail,
//! a document or a chat keeps its headings, lists, tables and styles.
//! Apps that take only text take the Markdown beside it.

use crate::doc::{Block, Kind, ListStyle, from_markdown, parse_blocks};
use crate::inline;

/// `md` as an HTML fragment. `image_url` turns an image's `src` into
/// something the pasting app can load (a `file:///` URL for a file); it
/// is told when the `src` is an Obsidian embed's (`![[…]]`), a name to
/// look for ([`find_picture`](crate::doc::find_picture)).
pub fn html(md: &str, image_url: impl Fn(&str, bool) -> String) -> String {
    render(&parse_blocks(md), image_url, false)
}

/// `md` as the viewer shows it (the PWA, docs/SYNC.md §8): what [`html`]
/// gives, and more of what the desktop draws: real checkboxes, each with
/// `data-task="n"` (its place among the note's checkboxes, from 0) so a
/// tap names it; highlights as `<mark>` (a colour of its own as its
/// `--mark`), the words' colours as `--ink`, for the page's styles to draw
/// in the theme's hues. An image's `src` is whatever `image_url` makes of
/// it, as for [`html`].
///
/// The note is read whole, as the desktop reads it: its title is its
/// `<h1>`, and front matter is not shown.
pub fn viewer(md: &str, image_url: impl Fn(&str, bool) -> String) -> String {
    let (title, doc) = from_markdown(md);
    let mut out = title.map(|t| format!("<h1>{}</h1>", spans(&t, true))).unwrap_or_default();
    let blocks: Vec<Block> = doc.blocks().iter().filter(|b| !matches!(&b.kind, Kind::Raw(r) if r.starts_with("---\n"))).map(|b| (**b).clone()).collect();
    out.push_str(&render(&blocks, image_url, true));
    out
}

fn render(blocks: &[Block], image_url: impl Fn(&str, bool) -> String, viewer: bool) -> String {
    let spans = |t: &str| spans(t, viewer);
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
    for b in blocks {
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
                out.push_str("<table border=\"1\" cellspacing=\"0\" cellpadding=\"6\">");
                for (r, row) in t.rows.iter().enumerate() {
                    out.push_str("<tr>");
                    let cell = if r == 0 { "th" } else { "td" };
                    for c in &row.cells {
                        out.push_str(&format!("<{cell}>{}</{cell}>", spans(c)));
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</table>");
            }
            Kind::Divider => out.push_str("<hr>"),
            Kind::Image { src, alt, width, float, wiki } => {
                let mut w = width.map(|w| format!(" width=\"{w}\"")).unwrap_or_default();
                match float {
                    crate::doc::Float::Left => w.push_str(" align=\"left\""),
                    crate::doc::Float::Right => w.push_str(" align=\"right\""),
                    crate::doc::Float::None => {}
                }
                out.push_str(&format!(
                    "<p><img src=\"{}\" alt=\"{}\"{w}></p>",
                    escape(&image_url(src, *wiki)),
                    escape(alt)
                ));
            }
            Kind::Raw(t) => {
                // A fence's body, without the fences.
                let body: Vec<&str> = t.lines().collect();
                let inner = if body.len() >= 2 && body[0].trim_start().starts_with(['`', '~']) {
                    body[1..body.len() - 1].join("\n")
                } else {
                    t.clone()
                };
                out.push_str(&format!("<pre><code>{}</code></pre>", escape(&inner)));
            }
            Kind::Item { .. } => {}
        }
    }
    close_to(&mut out, &mut lists, 0);
    out
}

/// One line's inline styles as tags, its markers left out; with `viewer`,
/// highlights and colours too.
fn spans(text: &str, viewer: bool) -> String {
    let mut out = String::new();
    // A link's target is a marker run between its text and the next text.
    let runs = inline::runs(text);
    let mut i = 0;
    while i < runs.len() {
        let (a, b, s) = runs[i];
        if s.marker {
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
        if viewer && s.ink != 0 {
            piece = format!("<span class=\"ink\" style=\"--ink:#{:08x}\">{piece}</span>", s.ink);
        }
        if viewer && s.mark == inline::HIGHLIGHT {
            piece = format!("<mark>{piece}</mark>");
        } else if viewer && s.mark != 0 {
            piece = format!("<mark style=\"--mark:#{:08x}\">{piece}</mark>", s.mark);
        }
        if s.link {
            // `[text](url)`: the url is what follows `](`.
            let url = text[b..]
                .strip_prefix("](")
                .and_then(|r| r.split_once(')'))
                .map(|(u, _)| u.to_string());
            if let Some(url) = url {
                piece = format!("<a href=\"{}\">{piece}</a>", escape(&url));
            }
        }
        out.push_str(&piece);
        i += 1;
    }
    out
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

    #[test]
    fn the_viewer_draws_checkboxes_highlights_and_colours() {
        let md = "- [ ] milk\n- [x] bread\n  - [ ] rye\n\nSee ==this== and <mark style=\"background:#ffd0d0\">that</mark>, <span style=\"color:#c03030\">red</span>.";
        let h = viewer(md, |s, _| format!("blob:{s}"));
        assert!(h.contains("<li class=\"task\"><input type=\"checkbox\" data-task=\"0\"><span>milk</span></li>"), "{h}");
        assert!(h.contains("<li class=\"task done\"><input type=\"checkbox\" data-task=\"1\" checked><span>bread</span></li>"), "{h}");
        assert!(h.contains("data-task=\"2\"><span>rye"), "{h}");
        assert!(h.contains("<mark>this</mark>"), "{h}");
        assert!(h.contains("<mark style=\"--mark:#"), "{h}");
        assert!(h.contains("<span class=\"ink\" style=\"--ink:#c03030ff\">red</span>"), "{h}");
        // Front matter isn't drawn, nor counted: box 0 is the first real
        // one, as toggle_task counts.
        let fm = "---\ntags: [x]\n- [ ] in front matter\n---\n# Title\n\n- [ ] real\n";
        let v = viewer(fm, |s, _| s.to_string());
        assert!(v.starts_with("<h1>Title</h1>") && v.contains("data-task=\"0\"><span>real") && !v.contains("front matter"), "{v}");
        assert_eq!(crate::doc::toggle_task(fm, 0).unwrap(), fm.replacen("- [ ] real", "- [x] real", 1));
        // The copy for mail stays as it was.
        assert!(html(md, |s, _| s.to_string()).contains("\u{2610} milk"));
    }
}
