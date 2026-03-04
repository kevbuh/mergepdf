# mergepdf

TUI tool for merging PDFs. Browse folders, select files, merge.

## Install

```bash
cargo install --path .
```

Requires [Ghostscript](https://www.ghostscript.com/) (`gs`) on your PATH.

## Usage

```bash
mergepdf
```

Navigate folders → check/uncheck PDFs → name output → merge.

### Controls

| Key | Action |
|-----|--------|
| `↑/↓` `j/k` | Navigate |
| `enter` | Open folder / confirm |
| `s` | Select folder |
| `backspace` | Parent directory |
| `space` | Toggle file |
| `a` | Toggle all |
| `esc` | Back / quit |
