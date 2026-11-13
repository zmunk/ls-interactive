use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
    process::Command,
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
    text::Line,
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

fn load_rows(directory: &Path, show_hidden: bool) -> Result<Vec<Line<'static>>, Box<dyn Error>> {
    let mut command = Command::new("eza");
    command
        .current_dir(directory)
        .args(["--classify=always", "-l", "--color=always"]);
    if show_hidden {
        command.arg("-a");
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
    Ok(output.stdout.into_text()?.lines)
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
    let mut rows = load_rows(&directory, show_hidden)?;

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

    loop {
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
            frame.render_widget(
                Paragraph::new(
                    "j/k: move  Enter: open  Bksp: parent  .: hidden  e: nvim  q/Ctrl-C: quit",
                )
                .style(Style::default().fg(Color::DarkGray)),
                hints,
            );
            frame.render_widget(
                Paragraph::new(directory.to_string_lossy()).style(Style::default().fg(Color::Cyan)),
                cwd,
            );
        })?;

        if let Event::Key(key) = event::read()? {
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                continue;
            }
            let current = selection.selected().unwrap_or(0);
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('.') => {
                    let previous = rows
                        .get(current)
                        .map(|line| entry_for_row(&directory, line))
                        .transpose()?
                        .flatten();
                    show_hidden = !show_hidden;
                    rows = load_rows(&directory, show_hidden)?;
                    let selected = previous.and_then(|path| {
                        rows.iter().position(|line| {
                            entry_for_row(&directory, line).ok().flatten().as_ref() == Some(&path)
                        })
                    });
                    selection = ListState::default()
                        .with_selected(selected.or_else(|| (!rows.is_empty()).then_some(0)));
                    resize_listing(&mut terminal, rows.len())?;
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
                            rows = load_rows(&directory, show_hidden)?;
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
                        let next_rows = load_rows(&destination, show_hidden)?;
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
