# zellij-cwd-plugin

A Zellij plugin that automatically renames tabs based on the current working directory of the focused pane.

## Features

- **Automatic tab renaming** based on the current working directory
- **Process name mode** - optionally show the running command instead of CWD (ignores shell names)
- **Multiple path formats**:
  - `basename` - just the directory name (default)
  - `full` - complete path
  - `tilde` - path with home directory replaced by `~`
  - `segments:N` - last N path segments
- **Configurable truncation** - truncate long names from left or right
- **Path exclusions** - ignore specific directories
- **Custom prefix/suffix** - wrap tab names with custom strings

## Requirements

- **Zellij built from `main` branch** - this plugin uses the `CwdChanged` event introduced in [PR #4546](https://github.com/zellij-org/zellij/pull/4546)
- **Rust toolchain** with `wasm32-wasip1` target

## Installation

### Build from source

```bash
# Add the WASM target
rustup target add wasm32-wasip1

# Clone and build
git clone https://github.com/YOUR_USERNAME/zellij-cwd-plugin.git
cd zellij-cwd-plugin
cargo build --release
```

The compiled plugin will be at:
```
target/wasm32-wasip1/release/zellij_cwd_plugin.wasm
```

### Add to your Zellij config

Copy the `.wasm` file to your Zellij plugins directory (e.g., `~/.config/zellij/plugins/`) and add it to your layout:

```kdl
layout {
    // ... your panes ...

    pane size=1 borderless=true {
        plugin location="file:~/.config/zellij/plugins/zellij_cwd_plugin.wasm"
    }
}
```

## Configuration

All options are configured within the plugin block in your KDL layout file:

```kdl
plugin location="file:path/to/zellij_cwd_plugin.wasm" {
    source "cwd"              // "cwd" or "process"
    format "basename"         // "basename", "full", "tilde", or "segments:N"
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
| `max_length` | number | `0` | Maximum tab name length (0 = unlimited) |
| `truncate_side` | `left`, `right` | `right` | Which side to truncate when name exceeds max_length |
| `prefix` | string | `""` | String to prepend to tab name |
| `suffix` | string | `""` | String to append to tab name |
| `exclude` | paths | `""` | Colon-separated list of paths to ignore |

## Usage Examples

### Minimal (defaults)

```kdl
pane size=1 borderless=true {
    plugin location="file:zellij_cwd_plugin.wasm"
}
```

Shows the directory basename (e.g., `myproject`).

### Show last 2 path segments

```kdl
plugin location="file:zellij_cwd_plugin.wasm" {
    format "segments:2"
}
```

Shows `projects/myproject` instead of just `myproject`.

### Path with tilde and truncation

```kdl
plugin location="file:zellij_cwd_plugin.wasm" {
    format "tilde"
    max_length "20"
    truncate_side "left"
}
```

Shows `...cts/myproject` for long paths like `~/dev/projects/myproject`.

### Process name with fallback

```kdl
plugin location="file:zellij_cwd_plugin.wasm" {
    source "process"
}
```

Shows `vim` when editing, `cargo` when building, but falls back to CWD when the shell (bash, zsh, fish, etc.) is in the foreground.

### Ignore temporary directories

```kdl
plugin location="file:zellij_cwd_plugin.wasm" {
    exclude "/tmp:/var:/proc"
}
```

Won't rename tabs when focused pane is in these directories.

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
4. Tab names are cached to avoid redundant rename calls

## License

MIT
