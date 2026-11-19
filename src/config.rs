use std::{env, fs, io, path::PathBuf};

use ratatui::style::{Color, Modifier, Style};
use toml::{Table, Value};

pub(super) struct Config {
    pub(super) jump_hint_style: Style,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            jump_hint_style: Style::default()
                .fg(Color::Rgb(0xFF, 0xB4, 0x54))
                .bg(Color::Reset)
                .add_modifier(Modifier::BOLD),
        }
    }
}

impl Config {
    pub(super) fn load() -> io::Result<Self> {
        let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) else {
            return Ok(Self::default());
        };
        let path = PathBuf::from(home).join(".config/ls-interactive/config.toml");
        let result = match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => Err(err),
        };
        result.map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", path.display())))
    }

    fn parse(text: &str) -> io::Result<Self> {
        let table: Table = text.parse().map_err(invalid_data)?;
        let mut config = Self::default();
        if let Some(section) = table.get("jump_hints") {
            let section = section
                .as_table()
                .ok_or_else(|| invalid_data("jump_hints must be a table"))?;
            if let Some(value) = section.get("fg") {
                config.jump_hint_style = config.jump_hint_style.fg(parse_color("fg", value)?);
            }
            if let Some(value) = section.get("bg") {
                config.jump_hint_style = config.jump_hint_style.bg(parse_color("bg", value)?);
            }
        }
        Ok(config)
    }
}

fn invalid_data(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn parse_color(key: &str, value: &Value) -> io::Result<Color> {
    value
        .as_str()
        .filter(|value| value.is_ascii())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            invalid_data(format!(
                "jump_hints.{key} must be a color string (e.g. \"red\" or \"#ff0000\")"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_independent_color_overrides() {
        let defaults = Config::parse("").unwrap().jump_hint_style;
        assert_eq!(defaults.fg, Some(Color::Rgb(0xFF, 0xB4, 0x54)));
        assert_eq!(defaults.bg, Some(Color::Reset));
        let custom = Config::parse("[jump_hints]\nfg = 'red'\n")
            .unwrap()
            .jump_hint_style;
        assert_eq!(custom, defaults.fg(Color::Red));
        let custom = Config::parse("[jump_hints]\nbg = '#102030'\n")
            .unwrap()
            .jump_hint_style;
        assert_eq!(custom, defaults.bg(Color::Rgb(16, 32, 48)));
    }

    #[test]
    fn rejects_invalid_colors_and_toml() {
        for text in [
            "[jump_hints]\nfg = 'not-a-color'",
            "[jump_hints]\nbg = true",
            "[",
        ] {
            assert!(Config::parse(text).is_err());
        }
    }
}
