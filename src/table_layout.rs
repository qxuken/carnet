//! Table columns share the available room according to visible content.

use carnet_core::{doc::Table, inline};
use kui_native::TextStyle;
use std::collections::HashMap;

#[derive(Default)]
pub struct Cache(HashMap<u64, Measured>);

struct Measured {
    table: Table,
    pad: f32,
    scale: f32,
    styles: [TextStyle; 2],
    natural: Natural,
}

struct Natural {
    widths: Vec<f32>,
    minimums: Vec<f32>,
    cells: Vec<Vec<(f32, f32)>>,
}

impl Cache {
    /// Text and font metrics determine natural widths; resizing only fits
    /// those measurements to the new room. Keep a snapshot so undo, edits
    /// and another note with the same block id cannot reuse stale widths.
    #[allow(clippy::too_many_arguments)]
    pub fn columns(&mut self, id: u64, table: &Table, available: f32, pad: f32, scale: f32, scroll: bool, styles: [TextStyle; 2], measure: impl FnMut(&str, bool) -> f32) -> Vec<f32> {
        let fresh = self.0.get(&id).is_some_and(|m| m.table == *table && m.pad == pad && m.scale == scale && m.styles == styles);
        if !fresh {
            let previous = self.0.get(&id).filter(|m| m.pad == pad && m.scale == scale && m.styles == styles);
            let natural = measure_columns(table, pad, scale, previous, measure);
            self.0.insert(id, Measured { table: table.clone(), pad, scale, styles, natural });
        }
        let m = &self.0[&id];
        fit(m.natural.widths.clone(), &m.natural.minimums, available, scroll)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(u64) -> bool) {
        self.0.retain(|id, _| keep(*id));
    }
}

#[cfg(test)]
pub fn columns(table: &Table, available: f32, pad: f32, scale: f32, scroll: bool, measure: impl FnMut(&str, bool) -> f32) -> Vec<f32> {
    let natural = measure_columns(table, pad, scale, None, measure);
    fit(natural.widths, &natural.minimums, available, scroll)
}

fn measure_columns(table: &Table, pad: f32, scale: f32, previous: Option<&Measured>, mut measure: impl FnMut(&str, bool) -> f32) -> Natural {
    let mut widths = vec![48.0 * scale; table.width()];
    let mut minimums = widths.clone();
    let mut cells = vec![Vec::new(); table.rows.len()];
    // Bound shaping work on very large tables and unusually long cells.
    // Include rows throughout the table, so a long entry near its end
    // still contributes. Editing any sampled cell recomputes its width.
    let step = table.rows.len().div_ceil(64).max(1);
    for (r, row) in table.rows.iter().enumerate().filter(|(r, _)| *r == 0 || r % step == 0) {
        let header = table.is_header(r);
        for (c, cell) in row.cells.iter().enumerate() {
            // A keystroke changes one cell. Reuse the others' metrics,
            // then reduce all cells again so a formerly widest cell may
            // shrink, and row/column changes still produce exact maxima.
            let cached = previous.and_then(|old| {
                if old.table.is_header(r) != header || old.table.cols.get(c) != table.cols.get(c) { return None; }
                let old_row = old.table.rows.get(r)?;
                if old_row.id != row.id || old_row.cells.get(c) != Some(cell) { return None; }
                old.natural.cells.get(r)?.get(c).copied()
            });
            let (width, minimum) = cached.unwrap_or_else(|| {
                let text: String = inline::plain(cell).chars().take(200).collect();
                let cap = if header { 120.0 } else { 300.0 } * scale;
                let content = if inline::cell_pictures(cell).is_empty() {
                    measure(&text, header).min(cap)
                } else {
                    measure(&text, header).min(cap).max(180.0 * scale)
                };
                let word = text.split_whitespace().max_by_key(|word| word.chars().count()).unwrap_or("");
                let minimum = (48.0 * scale).max(measure(word, header).min(140.0 * scale) + 2.0 * pad);
                ((content + 2.0 * pad).max(minimum), minimum)
            });
            cells[r].push((width, minimum));
            minimums[c] = minimums[c].max(minimum);
            widths[c] = widths[c].max(width);
        }
    }
    Natural { widths, minimums, cells }
}

fn fit(mut widths: Vec<f32>, minimums: &[f32], available: f32, scroll: bool) -> Vec<f32> {
    let wanted: f32 = widths.iter().sum();
    let minimum: f32 = minimums.iter().sum();
    // Only Scroll may exceed its pane. If word minimums cannot fit in
    // Page or Window, shrink them together, keeping their proportions.
    let room = if scroll { wanted } else { available.min(wanted) }.max(1.0);
    if minimum > room {
        return minimums.iter().map(|min| min * room / minimum).collect();
    }
    let extra: f32 = widths.iter().zip(minimums).map(|(w, min)| w - min).sum();
    let left = room - minimum;
    let count = widths.len().max(1) as f32;
    for (w, min) in widths.iter_mut().zip(minimums) {
        *w = min + if extra > 0.0 { left * (*w - min) / extra } else { left / count };
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
        assert!(fit.iter().sum::<f32>() < 400.0, "short tables keep their natural width even in a wide pane");
        let wide = columns(&table, 200.0, 14.0, 1.0, true, measure);
        assert!(wide.iter().sum::<f32>() > 200.0);
        let narrow = columns(&table, 90.0, 14.0, 1.0, false, measure);
        assert!(narrow.iter().all(|w| *w > 0.0));
        assert!((narrow.iter().sum::<f32>() - 90.0).abs() < 0.01, "bounded tables always fit");
    }

    #[test]
    fn unchanged_tables_and_resizes_reuse_measurements_but_edits_and_fonts_do_not() {
        let mut cache = Cache::default();
        let mut table = Table::from_cells(vec![vec!["Title".into(), "Author".into()], vec!["Book".into(), "Name".into()]]);
        let styles = [TextStyle::new(16.0), TextStyle::new(12.0)];
        let measure = |s: &str, _: bool| s.len() as f32 * 8.0;
        let original = cache.columns(1, &table, 600.0, 14.0, 1.0, false, styles, measure);
        let no_measure = |_: &str, _: bool| panic!("unchanged content must not be remeasured");
        assert_eq!(cache.columns(1, &table, 600.0, 14.0, 1.0, false, styles, no_measure), original);
        let narrow = cache.columns(1, &table, 80.0, 14.0, 1.0, false, styles, no_measure);
        assert!((narrow.iter().sum::<f32>() - 80.0).abs() < 0.01);
        assert!(cache.columns(1, &table, 80.0, 14.0, 1.0, true, styles, no_measure).iter().sum::<f32>() > 80.0);
        table.rows[1].cells[0] = "A much longer title for this book".into();
        let mut edited_calls = 0;
        let edited = cache.columns(1, &table, 600.0, 14.0, 1.0, false, styles, |s, h| { edited_calls += 1; measure(s, h) });
        assert!(edited[0] > original[0]);
        assert_eq!(edited_calls, 2, "typing remeasures only the edited cell's text and word minimum");
        let mut calls = 0;
        cache.columns(1, &table, 600.0, 14.0, 1.0, false, [TextStyle::new(20.0), styles[1]], |s, h| { calls += 1; measure(s, h) });
        assert!(calls > 0, "a font change must remeasure");
    }
}
