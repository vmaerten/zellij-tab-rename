# zellij-tab-rename

A Zellij plugin that automatically renames tabs based on the current working directory of the focused pane.

## Features

- **Automatic tab renaming** based on the current working directory
- **Git root mode** - show the git repository root name instead of the full CWD
- **Process name mode** - optionally show the running command instead of CWD (ignores shell names)
- **Multiple path formats**:
  - `basename` - just the directory name (default)
  - `full` - complete path
  - `tilde` - path with home directory replaced by `~`
  - `segments:N` - last N path segments
- **Configurable truncation** - truncate long names from left or right
- **Path exclusions** - ignore specific directories
- **Custom prefix/suffix** - wrap tab names with custom strings
- **Tab decorations via pipe** - dynamically add icons/indicators from external tools

## Requirements

- **Zellij built from `main` branch** - this plugin uses the `CwdChanged` event introduced in [PR #4546](https://github.com/zellij-org/zellij/pull/4546)
- **Rust toolchain** with `wasm32-wasip1` target

## Installation

### Build from source

```bash
# Add the WASM target
rustup target add wasm32-wasip1

# Clone and build
git clone https://github.com/vmaerten/zellij-tab-rename.git
cd zellij-tab-rename
cargo build --release --target wasm32-wasip1
```

The compiled plugin will be at:
```
target/wasm32-wasip1/release/zellij-tab-rename.wasm
```

### Add to your Zellij config

Copy the `.wasm` file to your Zellij plugins directory (e.g., `~/.config/zellij/plugins/`) and add it to your layout:

```kdl
layout {
    // ... your panes ...

    pane size=1 borderless=true {
        plugin location="file:~/.config/zellij/plugins/zellij-tab-rename.wasm"
    }
}
```

## Configuration

All options are configured within the plugin block in your KDL layout file:

```kdl
plugin location="file:path/to/zellij-tab-rename.wasm" {
    source "cwd"              // "cwd" or "process"
    format "basename"         // "basename", "full", "tilde", or "segments:N"
    git_root "true"           // show git repo root name instead of CWD
    max_length "25"           // 0 = unlimited
    truncate_side "right"     // "left" or "right"
    prefix ""                 // prefix string
    suffix ""                 // suffix string
    exclude "/tmp:/var"       // colon-separated paths to ignore
}
```

### Options Reference

| Option | Values | Default | Description |
|--------|--------|---------|-------------|
| `source` | `cwd`, `process` | `cwd` | `cwd` shows directory name, `process` shows running command (falls back to CWD for shells) |
| `format` | `basename`, `full`, `tilde`, `segments:N` | `basename` | How to format the path |
| `git_root` | `true`, `false` | `false` | When enabled, resolves the git repository root and displays the path relative to it (e.g., `proj/src` instead of `/home/user/dev/proj/src`) |
| `max_length` | number | `0` | Maximum tab name length (0 = unlimited) |
| `truncate_side` | `left`, `right` | `right` | Which side to truncate when name exceeds max_length |
| `prefix` | string | `""` | String to prepend to tab name |
| `suffix` | string | `""` | String to append to tab name |
| `exclude` | paths | `""` | Colon-separated list of paths to ignore |

## Usage Examples

### Minimal (defaults)

```kdl
pane size=1 borderless=true {
    plugin location="file:zellij-tab-rename.wasm"
}
```

Shows the directory basename (e.g., `myproject`).

### Git root with segments

```kdl
plugin location="file:zellij-tab-rename.wasm" {
    git_root "true"
    format "segments:2"
}
```

Shows `proj/src` when you're in `/home/user/dev/proj/src` (relative to git root).

### Show last 2 path segments

```kdl
plugin location="file:zellij-tab-rename.wasm" {
    format "segments:2"
}
```

Shows `projects/myproject` instead of just `myproject`.

### Path with tilde and truncation

```kdl
plugin location="file:zellij-tab-rename.wasm" {
    format "tilde"
    max_length "20"
    truncate_side "left"
}
```

Shows `...cts/myproject` for long paths like `~/dev/projects/myproject`.

### Process name with fallback

```kdl
plugin location="file:zellij-tab-rename.wasm" {
    source "process"
}
```

Shows `vim` when editing, `cargo` when building, but falls back to CWD when the shell (bash, zsh, fish, etc.) is in the foreground.

### Ignore temporary directories

```kdl
plugin location="file:zellij-tab-rename.wasm" {
    exclude "/tmp:/var:/proc"
}
```

Won't rename tabs when focused pane is in these directories.

## Tab Decorations (Pipe Protocol)

The plugin accepts pipe messages to dynamically add prefix/suffix decorations to tab names. This is useful for external tools (CI indicators, git status, etc.) to enrich tab names without interfering with the CWD-based renaming.

Decorations wrap the tab name: `{decoration prefix}{tab name}{decoration suffix}`

### Commands

Set a prefix on a tab (by pane ID):
```bash
echo "🔨 " | zellij pipe set_prefix --args "pane=42"
```

Set a suffix on the focused tab:
```bash
echo " ✓" | zellij pipe set_suffix --args "tab=focused"
```

Clear decorations for a specific pane's tab:
```bash
zellij pipe clear --args "pane=42"
```

Clear all decorations on all tabs:
```bash
zellij pipe clear
```

### Notes

- Decorations are **not truncated** — only the base name is subject to `max_length`
- When the source pane disappears, its decorations are automatically cleaned up
- When a tab is closed, its decorations are removed

## Development

### Running tests

Tests run on the native target (not WASM):

```bash
cargo test --target x86_64-unknown-linux-gnu
```

### Development layout

A development layout is included in `zellij.kdl`:

```bash
zellij --layout zellij.kdl
```

This opens the source file, a build pane, and loads the plugin from the debug build.

## How it works

1. The plugin subscribes to `CwdChanged`, `TabUpdate`, and `PaneUpdate` events
2. It tracks which pane is focused in each tab
3. When the CWD changes in the focused pane, it renames the tab accordingly
4. Optionally resolves the git root via `git rev-parse --show-toplevel` (async, cached)
5. Tab names are cached to avoid redundant rename calls
6. External tools can add decorations via the pipe protocol

## License

MIT
