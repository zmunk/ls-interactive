# ls-interactive

## Extra `eza` flags

Command-line arguments are passed directly to `eza` on every listing refresh. For example:

```sh
ls-interactive --group-directories-first --ignore-glob '*.tmp'
```

Use flags that preserve the long, one-entry-per-line output. Avoid layout modes such as `--tree` and path operands; the app manages which directory is listed.

## Configuration

Create `~/.config/ls-interactive/config.toml` to customize the colors of Jump Hints (`/`):

```toml
[jump_hints]
fg = "#FFB454"
bg = "reset"
```

These are the defaults. Colors can be named (e.g. `"magenta"`) or written as `"#RRGGBB"`. Use
`"reset"` for the terminal's default color. Restart the app to apply changes.
