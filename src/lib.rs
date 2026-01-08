use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use zellij_tile::prelude::*;

// === Configuration Enums ===

#[derive(Default, PartialEq)]
enum Source {
    #[default]
    Cwd,
    Process,
}

#[derive(Default, PartialEq)]
enum Format {
    #[default]
    Basename,
    Full,
    Tilde,
    Segments(usize),
}

#[derive(Default, PartialEq)]
enum TruncateSide {
    Left,
    #[default]
    Right,
}

// === Configuration Struct ===

#[derive(Default)]
struct Config {
    source: Source,
    format: Format,
    max_length: usize, // 0 = unlimited
    truncate_side: TruncateSide,
    prefix: String,
    suffix: String,
    excludes: Vec<PathBuf>,
    home_dir: Option<PathBuf>,
}

impl Config {
    fn from_configuration(configuration: &std::collections::BTreeMap<String, String>) -> Self {
        let source = match configuration.get("source").map(|s| s.as_str()) {
            Some("process") => Source::Process,
            _ => Source::Cwd,
        };

        let format = match configuration.get("format").map(|s| s.as_str()) {
            Some("full") => Format::Full,
            Some("tilde") => Format::Tilde,
            Some(s) if s.starts_with("segments:") => {
                let n = s[9..].parse().unwrap_or(1);
                Format::Segments(n)
            }
            _ => Format::Basename,
        };

        let max_length = configuration
            .get("max_length")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let truncate_side = match configuration.get("truncate_side").map(|s| s.as_str()) {
            Some("left") => TruncateSide::Left,
            _ => TruncateSide::Right,
        };

        let prefix = configuration
            .get("prefix")
            .cloned()
            .unwrap_or_default();

        let suffix = configuration
            .get("suffix")
            .cloned()
            .unwrap_or_default();

        let excludes = configuration
            .get("exclude")
            .map(|s| s.split(':').map(PathBuf::from).collect())
            .unwrap_or_default();

        let home_dir = configuration
            .get("home_dir")
            .map(PathBuf::from)
            .or_else(|| std::env::var("HOME").ok().map(PathBuf::from));

        Config {
            source,
            format,
            max_length,
            truncate_side,
            prefix,
            suffix,
            excludes,
            home_dir,
        }
    }
}

// === Pane State ===

#[derive(Default)]
struct PaneState {
    tab_index: usize,
    cwd: PathBuf,
    title: String,
}

// === Plugin State ===

#[derive(Default)]
struct State {
    config: Config,
    focused_panes: HashMap<usize, PaneId>,
    pane_info: HashMap<PaneId, PaneState>,
    current_tab_names: HashMap<usize, String>,
    got_permissions: bool,
}

register_plugin!(State);

// Shell names for detection (static to avoid allocations)
const SHELL_NAMES: &[&str] = &[
    "bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "csh", "nu", "nushell",
];

impl ZellijPlugin for State {
    fn load(&mut self, configuration: std::collections::BTreeMap<String, String>) {
        self.config = Config::from_configuration(&configuration);

        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);
        subscribe(&[
            EventType::CwdChanged,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(result) => {
                if result == PermissionStatus::Granted {
                    self.got_permissions = true;
                }
            }
            Event::CwdChanged(pane_id, cwd, _focused_clients) => {
                if self.got_permissions {
                    self.handle_cwd_changed(pane_id, cwd);
                }
            }
            Event::TabUpdate(tabs) => {
                if self.got_permissions {
                    self.handle_tab_update(&tabs);
                }
            }
            Event::PaneUpdate(pane_manifest) => {
                if self.got_permissions {
                    self.handle_pane_update(pane_manifest);
                }
            }
            _ => {}
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    fn handle_cwd_changed(&mut self, pane_id: PaneId, cwd: PathBuf) {
        if let Some(pane_state) = self.pane_info.get_mut(&pane_id) {
            pane_state.cwd = cwd;
            let tab_index = pane_state.tab_index;

            if self.focused_panes.get(&tab_index) == Some(&pane_id) {
                self.update_tab_name(tab_index, &pane_id);
            }
        }
    }

    fn handle_tab_update(&mut self, tabs: &[TabInfo]) {
        let active_positions: HashSet<usize> = tabs.iter().map(|t| t.position).collect();

        self.current_tab_names
            .retain(|k, _| active_positions.contains(k));
        self.focused_panes
            .retain(|k, _| active_positions.contains(k));

        // Clean up pane_info for panes in tabs that no longer exist
        self.pane_info
            .retain(|_, state| active_positions.contains(&state.tab_index));
    }

    fn handle_pane_update(&mut self, manifest: PaneManifest) {
        // Track which panes we see in this update
        let mut seen_panes: HashSet<PaneId> = HashSet::new();

        for (tab_index, panes) in &manifest.panes {
            for pane in panes {
                let pane_id = if pane.is_plugin {
                    PaneId::Plugin(pane.id)
                } else {
                    PaneId::Terminal(pane.id)
                };

                seen_panes.insert(pane_id);

                // Update or insert pane state
                let pane_state = self.pane_info.entry(pane_id).or_default();

                // Only update if changed to avoid unnecessary work
                if pane_state.tab_index != *tab_index {
                    pane_state.tab_index = *tab_index;
                }
                if pane_state.title != pane.title {
                    pane_state.title.clone_from(&pane.title);
                }

                if pane.is_focused && !pane.is_plugin {
                    let prev_focused = self.focused_panes.insert(*tab_index, pane_id);

                    if prev_focused != Some(pane_id) {
                        self.update_tab_name(*tab_index, &pane_id);
                    }
                }
            }
        }

        // Remove panes that are no longer present
        self.pane_info.retain(|id, _| seen_panes.contains(id));
    }

    fn update_tab_name(&mut self, tab_index: usize, pane_id: &PaneId) {
        let name = if let Some(pane_state) = self.pane_info.get(pane_id) {
            // Check exclusions
            if self.should_exclude(&pane_state.cwd) {
                return;
            }

            match self.config.source {
                Source::Process => {
                    if !pane_state.title.is_empty() && !Self::is_shell_name(&pane_state.title) {
                        pane_state.title.clone()
                    } else {
                        self.format_path(&pane_state.cwd)
                    }
                }
                Source::Cwd => self.format_path(&pane_state.cwd),
            }
        } else {
            return;
        };

        if name.is_empty() {
            return;
        }

        let final_name = if self.config.prefix.is_empty() && self.config.suffix.is_empty() {
            name
        } else {
            format!("{}{}{}", self.config.prefix, name, self.config.suffix)
        };

        let final_name = self.truncate_if_needed(&final_name);
        self.rename_tab_if_needed(tab_index, &final_name);
    }

    fn format_path(&self, path: &Path) -> String {
        if path.as_os_str().is_empty() {
            return String::new();
        }

        match &self.config.format {
            Format::Basename => path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("~")
                .to_string(),
            Format::Full => path.display().to_string(),
            Format::Tilde => self.replace_home_with_tilde(path),
            Format::Segments(n) => self.last_n_segments(path, *n),
        }
    }

    fn replace_home_with_tilde(&self, path: &Path) -> String {
        if let Some(home) = &self.config.home_dir {
            if let Ok(stripped) = path.strip_prefix(home) {
                return if stripped.as_os_str().is_empty() {
                    "~".to_string()
                } else {
                    format!("~/{}", stripped.display())
                };
            }
        }
        path.display().to_string()
    }

    fn last_n_segments(&self, path: &Path, n: usize) -> String {
        let components: Vec<_> = path.components().collect();
        let start = components.len().saturating_sub(n);

        let mut result = String::new();
        for (i, comp) in components[start..].iter().enumerate() {
            if i > 0 {
                result.push('/');
            }
            result.push_str(&comp.as_os_str().to_string_lossy());
        }
        result
    }

    fn truncate_if_needed(&self, s: &str) -> String {
        let char_count = s.chars().count();

        if self.config.max_length == 0 || char_count <= self.config.max_length {
            return s.to_string();
        }

        let max = self.config.max_length.saturating_sub(3); // Reserve space for "..."

        match self.config.truncate_side {
            TruncateSide::Right => {
                let truncated: String = s.chars().take(max).collect();
                format!("{}...", truncated)
            }
            TruncateSide::Left => {
                let truncated: String = s.chars().skip(char_count - max).collect();
                format!("...{}", truncated)
            }
        }
    }

    fn should_exclude(&self, path: &Path) -> bool {
        self.config
            .excludes
            .iter()
            .any(|excl| path.starts_with(excl))
    }

    fn is_shell_name(name: &str) -> bool {
        SHELL_NAMES
            .iter()
            .any(|shell| shell.eq_ignore_ascii_case(name))
    }

    fn rename_tab_if_needed(&mut self, tab_position: usize, new_name: &str) {
        let should_rename = self
            .current_tab_names
            .get(&tab_position)
            .is_none_or(|current| current != new_name);

        if should_rename && !new_name.is_empty() {
            rename_tab(tab_position as u32, new_name);
            self.current_tab_names
                .insert(tab_position, new_name.to_string());
        }
    }
}
