use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::{row_text, Row};

type Candidate = (String, Range<usize>);

pub(super) struct JumpMode {
    targets: HashMap<String, usize>,
    highlights: HashMap<usize, Range<usize>>,
}

impl JumpMode {
    pub(super) fn new(rows: &[Row], visible: Range<usize>, width: usize) -> Option<Self> {
        let candidates: Vec<_> = rows[visible.clone()]
            .iter()
            .map(|row| candidates_for_row(row, width))
            .collect();
        let mut owners = HashMap::new();
        for row in 0..candidates.len() {
            assign(row, &candidates, &mut owners, &mut HashSet::new());
        }
        if owners.is_empty() {
            return None;
        }
        let mut targets = HashMap::new();
        let mut highlights = HashMap::new();
        for (key, (row, choice)) in owners {
            let index = visible.start + row;
            targets.insert(key.to_owned(), index);
            highlights.insert(index, candidates[row][choice].1.clone());
        }
        Some(Self {
            targets,
            highlights,
        })
    }

    pub(super) fn target(&self, ch: char) -> Option<usize> {
        self.targets.get(&hint_key(ch)).copied()
    }

    pub(super) fn render_line(&self, index: usize, line: &Line<'static>) -> Line<'static> {
        let Some(range) = self.highlights.get(&index) else {
            return line.clone();
        };
        let mut line = line.clone();
        let mut offset = 0;
        for span in std::mem::take(&mut line.spans) {
            let span_end = offset + span.content.len();
            if offset < range.end && range.start < span_end {
                let start = range.start.saturating_sub(offset);
                let end = range.end.min(span_end) - offset;
                if start > 0 {
                    line.spans
                        .push(Span::styled(span.content[..start].to_owned(), span.style));
                }
                let hint_style = span.style.patch(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                );
                line.spans.push(Span::styled(
                    span.content[start..end].to_owned(),
                    hint_style,
                ));
                if end < span.content.len() {
                    line.spans
                        .push(Span::styled(span.content[end..].to_owned(), span.style));
                }
            } else {
                line.spans.push(span);
            }
            offset = span_end;
        }
        line
    }
}

fn hint_key(ch: char) -> String {
    ch.to_lowercase().collect()
}

fn candidates_for_row(row: &Row, width: usize) -> Vec<Candidate> {
    let Some(name) = &row.name else {
        return Vec::new();
    };
    let text = row_text(&row.line);
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for (offset, ch) in text[name.clone()].char_indices() {
        if !ch.is_alphabetic() {
            continue;
        }
        let start = name.start + offset;
        let end = start + ch.len_utf8();
        let end_column = Line::raw(&text[..end]).width();
        if end_column > width {
            break;
        }
        if end_column == Line::raw(&text[..start]).width() {
            continue;
        }
        let key = hint_key(ch);
        if seen.insert(key.clone()) {
            candidates.push((key, start..end));
        }
    }
    candidates
}

// Keep earlier rows matched. Try a free letter first, then look for a chain
// of reassignments that makes room for this row without dropping another.
fn assign<'a>(
    row: usize,
    candidates: &'a [Vec<Candidate>],
    owners: &mut HashMap<&'a str, (usize, usize)>,
    seen: &mut HashSet<&'a str>,
) -> bool {
    for (choice, (key, _)) in candidates[row].iter().enumerate() {
        if !owners.contains_key(key.as_str()) {
            owners.insert(key.as_str(), (row, choice));
            return true;
        }
    }
    for (choice, (key, _)) in candidates[row].iter().enumerate() {
        if seen.insert(key.as_str()) {
            let (owner, _) = owners[key.as_str()];
            if assign(owner, candidates, owners, seen) {
                owners.insert(key.as_str(), (row, choice));
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> Row {
        Row {
            line: Line::raw(format!("meta {name}/")),
            name: Some(5..5 + name.len()),
        }
    }

    #[test]
    fn reassigns_letters_and_favors_topmost_rows() {
        let rows = [row("ab"), row("BC"), row("a"), row("c")];
        let jump = JumpMode::new(&rows, 0..4, 80).unwrap();
        assert_eq!(jump.target('A'), Some(2));
        assert_eq!(jump.target('b'), Some(0));
        assert_eq!(jump.target('C'), Some(1));
        assert!(!jump.highlights.contains_key(&3));
    }

    #[test]
    fn excludes_offscreen_rows_metadata_and_clipped_letters() {
        let rows = [row("top"), row("éz"), row("bottom")];
        let jump = JumpMode::new(&rows, 1..2, 6).unwrap();
        assert_eq!(jump.target('É'), Some(1));
        for ch in ['t', 'b', 'm', 'z'] {
            assert_eq!(jump.target(ch), None);
        }
        assert!(JumpMode::new(&rows, 1..2, 5).is_none());
        assert!(JumpMode::new(&[row("123")], 0..1, 80).is_none());
    }

    #[test]
    fn highlights_only_the_assigned_letter_without_changing_text() {
        let rows = [row("ab"), row("a")];
        let jump = JumpMode::new(&rows, 0..2, 80).unwrap();
        let rendered = jump.render_line(0, &rows[0].line);
        assert_eq!(row_text(&rendered), row_text(&rows[0].line));
        assert_eq!(rendered.spans[1].content.as_ref(), "b");
        assert_eq!(rendered.spans[1].style.bg, Some(Color::Yellow));
    }
}
