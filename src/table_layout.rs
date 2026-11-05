//! Table columns share the available room according to visible content.

use carnet_core::{doc::Table, inline};

pub fn columns(table: &Table, available: f32, pad: f32, scale: f32, scroll: bool, mut measure: impl FnMut(&str, bool) -> f32) -> Vec<f32> {
    let mut widths = vec![48.0 * scale; table.width()];
    let mut minimums = widths.clone();
    // Bound shaping work on very large tables and unusually long cells.
    // Include rows throughout the table, so a long entry near its end
    // still contributes. Editing any sampled cell recomputes its width.
    let step = table.rows.len().div_ceil(64).max(1);
    for (r, row) in table.rows.iter().enumerate().filter(|(r, _)| *r == 0 || r % step == 0) {
        let header = table.is_header(r);
        for (c, cell) in row.cells.iter().enumerate() {
            let text: String = inline::plain(cell).chars().take(200).collect();
            let cap = if header { 120.0 } else { 300.0 } * scale;
            let content = if inline::cell_pictures(cell).is_empty() {
                measure(&text, header).min(cap)
            } else {
                measure(&text, header).min(cap).max(180.0 * scale)
            };
            let word = text.split_whitespace().max_by_key(|word| word.chars().count()).unwrap_or("");
            minimums[c] = minimums[c].max(measure(word, header).min(140.0 * scale) + 2.0 * pad);
            widths[c] = widths[c].max(content + 2.0 * pad).max(minimums[c]);
        }
    }
    let wanted: f32 = widths.iter().sum();
    let minimum: f32 = minimums.iter().sum();
    // Bounded tables wrap to their pane, but never crush columns into
    // single letters. If their word minimums cannot fit, the local
    // scroller is the fallback in every mode. Scroll keeps natural widths.
    let room = if scroll { available.max(wanted) } else { available.max(minimum) }.max(1.0);
    let extra: f32 = widths.iter().zip(&minimums).map(|(w, min)| w - min).sum();
    let left = room - minimum;
    for (w, min) in widths.iter_mut().zip(&minimums) {
        *w = min + if extra > 0.0 { left * (*w - min) / extra } else { left / table.width().max(1) as f32 };
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_size_columns_and_long_urls_do_not() {
        let table = Table::from_cells(vec![vec!["#".into(), "Title".into(), "Link".into()], vec!["1".into(), "A rather longer book title".into(), format!("[Lex](https://example.com/{})", "x".repeat(1000))]]);
        let measure = |s: &str, _: bool| s.chars().count() as f32 * 8.0;
        let fit = columns(&table, 600.0, 14.0, 1.0, false, measure);
        assert!(fit[1] > fit[2] * 2.0 && fit[2] > fit[0]);
        assert!((fit.iter().sum::<f32>() - 600.0).abs() < 0.01);
        let wide = columns(&table, 200.0, 14.0, 1.0, true, measure);
        assert!(wide.iter().sum::<f32>() > 200.0);
        let narrow = columns(&table, 90.0, 14.0, 1.0, false, measure);
        assert!(narrow.iter().all(|w| *w >= 48.0));
        assert!(narrow.iter().sum::<f32>() > 90.0, "a narrow pane scrolls instead of crushing words");
    }
}
