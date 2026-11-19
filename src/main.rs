use std::{
    error::Error,
    ffi::OsString,
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use ansi_to_tui::IntoText;
use crossterm::{
    cursor::{MoveToColumn, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, size},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListState, Paragraph},
    Terminal, TerminalOptions, Viewport,
};

mod config;
mod jump;

use config::Config;
use jump::JumpMode;

struct Row {
    line: Line<'static>,
    name: Option<Range<usize>>,
}

struct RestoreTerminal;

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show);
    }
}

fn load_rows(
    directory: &Path,
    show_hidden: bool,
    sort_modified: bool,
    eza_args: &[OsString],
) -> Result<Vec<Row>, Box<dyn Error>> {
    let mut command = Command::new("eza");
    command.current_dir(directory).args([
        "--classify=always",
        "-l",
        "--color=always",
        "--no-permissions",
        "--no-user",
    ]);
    command.args(eza_args);
    if show_hidden {
        command.arg("-a");
    }
    if sort_modified {
        command.args(["--sort=modified", "--reverse"]);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "eza failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    let mut rows = Vec::new();
    for mut line in output.stdout.into_text()?.lines {
        let entry = entry_and_name_for_row(directory, &row_text(&line))?;
        if entry.as_ref().is_some_and(|(path, _)| path.is_dir()) {
            for span in &mut line.spans {
                if let Some((index, ch)) = span
                    .content
                    .char_indices()
                    .find(|(_, ch)| !ch.is_whitespace())
                {
                    if ch == '-' {
                        span.content.to_mut().replace_range(index..index + 1, " ");
                    }
                    break;
                }
            }
        }
        rows.push(Row {
            line,
            name: entry.map(|(_, name)| name),
        });
    }
    Ok(rows)
}

fn directory_for_row(directory: &Path, line: &Line<'_>) -> io::Result<Option<PathBuf>> {
    Ok(entry_for_row(directory, line)?.filter(|path| path.is_dir()))
}

fn entry_for_row(directory: &Path, line: &Line<'_>) -> io::Result<Option<PathBuf>> {
    Ok(entry_and_name_for_row(directory, &row_text(line))?.map(|(path, _)| path))
}

fn row_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn entry_and_name_for_row(
    directory: &Path,
    text: &str,
) -> io::Result<Option<(PathBuf, Range<usize>)>> {
    let mut best: Option<(PathBuf, Range<usize>)> = None;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let displayed = if entry.file_type()?.is_symlink() {
            text.split_once(" -> ")
                .or_else(|| text.split_once(" ⇒ "))
                .or_else(|| text.split_once(" → "))
                .map_or(text, |(name, _)| name)
        } else {
            text
        };
        // Prefer the whole name over a suffix shared by another entry.
        if best
            .as_ref()
            .is_some_and(|(_, range)| range.len() > name.len())
        {
            continue;
        }
        if let Some(range) = displayed_name_range(displayed, &name) {
            best = Some((entry.path(), range));
        }
    }
    Ok(best)
}

fn displayed_name_range(displayed: &str, name: &str) -> Option<Range<usize>> {
    for marker in ["", "/", "@", "*"] {
        let Some(stem) = displayed.strip_suffix(marker) else {
            continue;
        };
        for quote in ["", "'", "\""] {
            let Some(prefix) = stem
                .strip_suffix(quote)
                .and_then(|stem| stem.strip_suffix(name))
                .and_then(|stem| stem.strip_suffix(quote))
            else {
                continue;
            };
            if prefix.ends_with(char::is_whitespace) {
                let start = prefix.len() + quote.len();
                return Some(start..start + name.len());
            }
        }
    }
    None
}

#[test]
fn filename_range_excludes_metadata_quotes_and_classification_markers() {
    let displayed = "meta 'some dir'/";
    let range = displayed_name_range(displayed, "some dir").unwrap();
    assert_eq!(&displayed[range], "some dir");
    assert!(displayed_name_range(displayed, "dir").is_none());
}

fn copy_path(path: &Path, absolute: bool) -> io::Result<Option<String>> {
    const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let text = if absolute {
        path.as_os_str()
    } else if let Some(name) = path.file_name() {
        name
    } else {
        return Ok(None);
    };
    let name = text.to_string_lossy();
    let mut encoded = String::new();
    for chunk in name.as_bytes().chunks(3) {
        let bits = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        encoded.push(BASE64[((bits >> 18) & 63) as usize] as char);
        encoded.push(BASE64[((bits >> 12) & 63) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            BASE64[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            BASE64[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b]52;c;{encoded}\x07")?;
    stdout.flush()?;
    Ok(Some(name.into_owned()))
}

fn listing_height(row_count: usize) -> io::Result<u16> {
    let (_, terminal_height) = size()?;
    Ok(row_count
        .saturating_add(2)
        .min(terminal_height.saturating_sub(1).max(3) as usize) as u16)
}

fn resize_listing(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    row_count: usize,
) -> io::Result<()> {
    let height = listing_height(row_count)?;
    let top = terminal.get_frame().area().y;
    terminal.clear()?;
    terminal.set_cursor_position((0, top))?;
    *terminal = Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )?;
    terminal.hide_cursor()
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;
    let mut directory = std::env::current_dir()?;
    let mut show_hidden = true;
    let mut sort_modified = false;
    let eza_args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut rows = load_rows(&directory, show_hidden, sort_modified, &eza_args)?;

    // An inline viewport uses the normal screen, not the alternate screen.
    // Long listings scroll inside the viewport as the selection moves.
    let height = listing_height(rows.len())?;
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )?;
    enable_raw_mode()?;
    let _restore = RestoreTerminal;
    terminal.hide_cursor()?;
    let mut selection = ListState::default().with_selected((!rows.is_empty()).then_some(0));
    let mut pending_g = false;
    let mut show_hints = false;
    let mut jump: Option<JumpMode> = None;
    let mut copied: Option<(String, Instant)> = None;

    loop {
        if copied
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            copied = None;
        }
        let mut visible = 0..0;
        let mut listing_width = 0;
        terminal.draw(|frame| {
            let [listing, hints, cwd] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            let list = List::new(rows.iter().enumerate().map(|(index, row)| {
                jump.as_ref().map_or_else(
                    || row.line.clone(),
                    |jump| jump.render_line(index, &row.line, config.jump_hint_style),
                )
            }))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(list, listing, &mut selection);
            let start = selection.offset().min(rows.len());
            visible = start..(start + usize::from(listing.height)).min(rows.len());
            listing_width = usize::from(listing.width);
            let hint_text = if jump.is_some() {
                "Jump: type a highlighted letter  Esc: cancel"
            } else if show_hints {
                "/: jump  gg: first  G: last  Enter: cd/nvim  Bksp: parent  .: hidden  m: modified/name  y/Y: copy name/path  e: nvim  q/Esc/Ctrl-C: quit  ?: hide hints"
            } else {
                "/: jump  ?: hints"
            };
            let mut footer = vec![Span::styled(
                hint_text,
                Style::default().fg(Color::DarkGray),
            )];
            if let Some((text, _)) = &copied {
                footer.push(Span::styled(
                    format!("  copied: {text}"),
                    Style::default().fg(Color::White),
                ));
            }
            frame.render_widget(Paragraph::new(Line::from(footer)), hints);
            frame.render_widget(
                Paragraph::new(directory.to_string_lossy()).style(Style::default().fg(Color::Cyan)),
                cwd,
            );
        })?;

        if let Some((_, until)) = &copied {
            if !event::poll(until.saturating_duration_since(Instant::now()))? {
                continue;
            }
        }
        let event = event::read()?;
        if matches!(event, Event::Resize(..)) {
            jump = None;
            continue;
        }
        if let Event::Key(key) = event {
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                continue;
            }
            if let Some(active) = &jump {
                match key.code {
                    KeyCode::Esc => jump = None,
                    KeyCode::Char(ch) if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
                        if let Some(row) = active.target(ch) {
                            selection.select(Some(row));
                            jump = None;
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let current = selection.selected().unwrap_or(0);
            if key.code == KeyCode::Char('g') {
                if pending_g {
                    selection.select((!rows.is_empty()).then_some(0));
                }
                pending_g = !pending_g;
                continue;
            }
            pending_g = false;
            match key.code {
                KeyCode::Char('/') if key.kind == KeyEventKind::Press => {
                    jump = JumpMode::new(&rows, visible, listing_width);
                }
                KeyCode::Char('?') => show_hints = !show_hints,
                KeyCode::Char('G') => {
                    selection.select(rows.len().checked_sub(1));
                }
                KeyCode::Char('u') | KeyCode::Char('d')
                    if key.modifiers.contains(KeyModifiers::CONTROL) && !rows.is_empty() =>
                {
                    let step =
                        (terminal.get_frame().area().height.saturating_sub(2) as usize / 2).max(1);
                    let next = if key.code == KeyCode::Char('u') {
                        current.saturating_sub(step)
                    } else {
                        current.saturating_add(step).min(rows.len() - 1)
                    };
                    selection.select(Some(next));
                }
                KeyCode::Char('q') => break,
                KeyCode::Esc if key.kind == KeyEventKind::Press => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('.') | KeyCode::Char('m') => {
                    let previous = rows
                        .get(current)
                        .map(|row| entry_for_row(&directory, &row.line))
                        .transpose()?
                        .flatten();
                    if key.code == KeyCode::Char('.') {
                        show_hidden = !show_hidden;
                    } else {
                        sort_modified = !sort_modified;
                    }
                    rows = load_rows(&directory, show_hidden, sort_modified, &eza_args)?;
                    let selected = previous.and_then(|path| {
                        rows.iter().position(|row| {
                            entry_for_row(&directory, &row.line).ok().flatten().as_ref()
                                == Some(&path)
                        })
                    });
                    selection = ListState::default()
                        .with_selected(selected.or_else(|| (!rows.is_empty()).then_some(0)));
                    resize_listing(&mut terminal, rows.len())?;
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(row) = rows.get(current) {
                        if let Some(path) = entry_for_row(&directory, &row.line)? {
                            if let Some(text) = copy_path(&path, key.code == KeyCode::Char('Y'))? {
                                copied = Some((text, Instant::now() + Duration::from_secs(1)));
                            }
                        }
                    }
                }
                KeyCode::Char('e') | KeyCode::Enter | KeyCode::Backspace => {
                    let going_up = key.code == KeyCode::Backspace;
                    let entry = if going_up {
                        None
                    } else if let Some(row) = rows.get(current) {
                        entry_for_row(&directory, &row.line)?
                    } else {
                        None
                    };
                    if let Some(path) = entry.as_ref().filter(|path| path.is_file()) {
                        disable_raw_mode()?;
                        terminal.show_cursor()?;
                        let result = Command::new("nvim")
                            .current_dir(&directory)
                            .arg("--")
                            .arg(path)
                            .status();
                        enable_raw_mode()?;
                        terminal.hide_cursor()?;
                        result?;
                        rows = load_rows(&directory, show_hidden, sort_modified, &eza_args)?;
                        let selected = rows.iter().position(|row| {
                            entry_for_row(&directory, &row.line).ok().flatten().as_ref()
                                == Some(path)
                        });
                        selection = ListState::default()
                            .with_selected(selected.or_else(|| (!rows.is_empty()).then_some(0)));
                        resize_listing(&mut terminal, rows.len())?;
                        continue;
                    }
                    let destination = match key.code {
                        KeyCode::Backspace => directory.parent().map(Path::to_path_buf),
                        KeyCode::Enter => entry.filter(|path| path.is_dir()),
                        _ => None,
                    };
                    if let Some(destination) = destination {
                        let next_rows =
                            load_rows(&destination, show_hidden, sort_modified, &eza_args)?;
                        let selected = if going_up {
                            next_rows.iter().position(|row| {
                                directory_for_row(&destination, &row.line)
                                    .ok()
                                    .flatten()
                                    .as_ref()
                                    == Some(&directory)
                            })
                        } else {
                            None
                        };
                        directory = destination.canonicalize()?;
                        rows = next_rows;
                        resize_listing(&mut terminal, rows.len())?;
                        selection = ListState::default()
                            .with_selected(selected.or_else(|| (!rows.is_empty()).then_some(0)));
                    }
                }
                KeyCode::Char('j') if !rows.is_empty() => {
                    selection.select(Some((current + 1).min(rows.len() - 1)));
                }
                KeyCode::Char('k') if !rows.is_empty() => {
                    selection.select(Some(current.saturating_sub(1)));
                }
                _ => {}
            }
        }
    }

    // Erase the footer, leaving the listing intact above the prompt.
    terminal.draw(|frame| {
        let area = frame.area();
        let [listing, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(area);
        let list = List::new(rows.iter().map(|row| row.line.clone()));
        frame.render_stateful_widget(list, listing, &mut selection);
        frame.render_widget(Clear, footer);
    })?;
    let bottom = terminal.get_frame().area().bottom().saturating_sub(2);
    terminal.set_cursor_position((0, bottom))?;
    execute!(terminal.backend_mut(), MoveToColumn(0))?;
    if let Some(file) = std::env::var_os("LS_INTERACTIVE_CWD_FILE") {
        std::fs::write(file, directory.as_os_str().as_encoded_bytes())?;
    }
    Ok(())
}
