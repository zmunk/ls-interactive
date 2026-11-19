use std::{
    error::Error,
    io::{self, Write},
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
) -> Result<Vec<Line<'static>>, Box<dyn Error>> {
    let mut command = Command::new("eza");
    command.current_dir(directory).args([
        "--classify=always",
        "-l",
        "--color=always",
        "--no-permissions",
        "--no-user",
    ]);
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
    let mut rows = output.stdout.into_text()?.lines;
    for row in &mut rows {
        if directory_for_row(directory, row)?.is_some() {
            for span in &mut row.spans {
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
    }
    Ok(rows)
}

fn directory_for_row(directory: &Path, line: &Line<'_>) -> io::Result<Option<PathBuf>> {
    Ok(entry_for_row(directory, line)?.filter(|path| path.is_dir()))
}

fn entry_for_row(directory: &Path, line: &Line<'_>) -> io::Result<Option<PathBuf>> {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let displayed = if entry.file_type()?.is_symlink() {
            text.split_once(" -> ")
                .or_else(|| text.split_once(" ⇒ "))
                .or_else(|| text.split_once(" → "))
                .map_or(text.as_str(), |(name, _)| name)
        } else {
            text.as_str()
        };
        let matched = [
            name.clone(),
            format!("'{name}'"),
            format!("\"{name}\""),
            format!("{name}/"),
            format!("'{name}'/"),
            format!("\"{name}\"/"),
            format!("{name}@"),
            format!("'{name}'@"),
            format!("\"{name}\"@"),
            format!("{name}*"),
            format!("'{name}'*"),
            format!("\"{name}\"*"),
        ]
        .iter()
        .any(|suffix| {
            displayed
                .strip_suffix(suffix.as_str())
                .is_some_and(|prefix| prefix.ends_with(char::is_whitespace))
        });
        if matched {
            matches.push((name.len(), entry.path()));
        }
    }
    // Prefer the whole name over a suffix shared by another directory.
    Ok(matches
        .into_iter()
        .max_by_key(|(length, _)| *length)
        .map(|(_, path)| path))
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
    let mut directory = std::env::current_dir()?;
    let mut show_hidden = true;
    let mut sort_modified = false;
    let mut rows = load_rows(&directory, show_hidden, sort_modified)?;

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
    let mut copied: Option<(String, Instant)> = None;

    loop {
        if copied
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            copied = None;
        }
        terminal.draw(|frame| {
            let [listing, hints, cwd] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            let list = List::new(rows.iter().cloned())
                .highlight_style(Style::default().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(list, listing, &mut selection);
            let hint_text = if show_hints {
                "gg: first  G: last  Enter: open  Bksp: parent  .: hidden  m: modified/name  y/Y: copy name/path  e: nvim  q/Ctrl-C: quit  ?: hide hints"
            } else {
                "?: hints"
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
        if let Event::Key(key) = event::read()? {
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
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
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('.') | KeyCode::Char('m') => {
                    let previous = rows
                        .get(current)
                        .map(|line| entry_for_row(&directory, line))
                        .transpose()?
                        .flatten();
                    if key.code == KeyCode::Char('.') {
                        show_hidden = !show_hidden;
                    } else {
                        sort_modified = !sort_modified;
                    }
                    rows = load_rows(&directory, show_hidden, sort_modified)?;
                    let selected = previous.and_then(|path| {
                        rows.iter().position(|line| {
                            entry_for_row(&directory, line).ok().flatten().as_ref() == Some(&path)
                        })
                    });
                    selection = ListState::default()
                        .with_selected(selected.or_else(|| (!rows.is_empty()).then_some(0)));
                    resize_listing(&mut terminal, rows.len())?;
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(line) = rows.get(current) {
                        if let Some(path) = entry_for_row(&directory, line)? {
                            if let Some(text) = copy_path(&path, key.code == KeyCode::Char('Y'))? {
                                copied = Some((text, Instant::now() + Duration::from_secs(1)));
                            }
                        }
                    }
                }
                KeyCode::Char('e') => {
                    if let Some(line) = rows.get(current) {
                        if let Some(path) = entry_for_row(&directory, line)?.filter(|p| p.is_file())
                        {
                            disable_raw_mode()?;
                            terminal.show_cursor()?;
                            let result = Command::new("nvim")
                                .current_dir(&directory)
                                .arg("--")
                                .arg(&path)
                                .status();
                            enable_raw_mode()?;
                            terminal.hide_cursor()?;
                            result?;
                            rows = load_rows(&directory, show_hidden, sort_modified)?;
                            let selected = rows.iter().position(|line| {
                                entry_for_row(&directory, line).ok().flatten().as_ref()
                                    == Some(&path)
                            });
                            selection = ListState::default().with_selected(
                                selected.or_else(|| (!rows.is_empty()).then_some(0)),
                            );
                            resize_listing(&mut terminal, rows.len())?;
                        }
                    }
                }
                KeyCode::Backspace | KeyCode::Enter => {
                    let going_up = key.code == KeyCode::Backspace;
                    let destination = if going_up {
                        directory.parent().map(Path::to_path_buf)
                    } else if let Some(line) = rows.get(current) {
                        directory_for_row(&directory, line)?
                    } else {
                        None
                    };
                    if let Some(destination) = destination {
                        let next_rows = load_rows(&destination, show_hidden, sort_modified)?;
                        let selected = if going_up {
                            next_rows.iter().position(|line| {
                                directory_for_row(&destination, line)
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
        let list = List::new(rows.iter().cloned());
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
