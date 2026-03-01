use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use zellij_tile::prelude::*;

// === Configuration Enums ===

#[derive(Default, Debug, PartialEq)]
enum Source {
    #[default]
    Cwd,
    Process,
}

#[derive(Default, Debug, PartialEq)]
enum Format {
    #[default]
    Basename,
    Full,
    Tilde,
    Segments(usize),
}

#[derive(Default, Debug, PartialEq)]
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
    git_root: bool,
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

        let git_root = configuration
            .get("git_root")
            .map(|s| s == "true")
            .unwrap_or(false);

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
            git_root,
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
    /// CWDs received before PaneUpdate (race condition on new tab / session restore)
    pending_cwds: HashMap<PaneId, PathBuf>,
    /// cwd -> Some(git_root) or None (not a git repo)
    git_root_cache: HashMap<PathBuf, Option<PathBuf>>,
    /// cwds with pending git lookups -> tab indices waiting for the result
    pending_git_lookups: HashMap<PathBuf, Vec<usize>>,
    /// Events received before permissions were granted
    buffered_events: Vec<Event>,
}

register_plugin!(State);

// Shell names for detection (static to avoid allocations)
const SHELL_NAMES: &[&str] = &[
    "bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "csh", "nu", "nushell",
];

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.config = Config::from_configuration(&configuration);
        eprintln!(
            "[cwd-plugin] load: source={:?}, format={:?}, git_root={}",
            self.config.source, self.config.format, self.config.git_root
        );

        let mut permissions = vec![
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ];
        let mut events = vec![
            EventType::CwdChanged,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
        ];
        if self.config.git_root {
            permissions.push(PermissionType::RunCommands);
            events.push(EventType::RunCommandResult);
        }
        request_permission(&permissions);
        subscribe(&events);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(result) => {
                eprintln!("[cwd-plugin] PermissionRequestResult: {:?}", result);
                if result == PermissionStatus::Granted {
                    self.got_permissions = true;
                    // Replay events that arrived before permissions
                    let buffered = std::mem::take(&mut self.buffered_events);
                    eprintln!("[cwd-plugin] replaying {} buffered events", buffered.len());
                    for ev in buffered {
                        self.process_event(ev);
                    }
                }
            }
            event => {
                if self.got_permissions {
                    self.process_event(event);
                } else {
                    eprintln!("[cwd-plugin] permissions pending, buffering event");
                    self.buffered_events.push(event);
                }
            }
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    fn process_event(&mut self, event: Event) {
        match event {
            Event::CwdChanged(pane_id, cwd, _) => {
                eprintln!("[cwd-plugin] event: CwdChanged pane={:?} cwd={}", pane_id, cwd.display());
                self.handle_cwd_changed(pane_id, cwd);
            }
            Event::TabUpdate(tabs) => {
                eprintln!("[cwd-plugin] event: TabUpdate ({} tabs)", tabs.len());
                self.handle_tab_update(&tabs);
            }
            Event::PaneUpdate(manifest) => {
                eprintln!("[cwd-plugin] event: PaneUpdate ({} tabs)", manifest.panes.len());
                self.handle_pane_update(manifest);
            }
            Event::RunCommandResult(exit_code, stdout, _stderr, context) => {
                eprintln!("[cwd-plugin] event: RunCommandResult exit={:?}", exit_code);
                self.handle_run_command_result(exit_code, stdout, context);
            }
            _ => {}
        }
    }

    fn handle_cwd_changed(&mut self, pane_id: PaneId, cwd: PathBuf) {
        if let Some(pane_state) = self.pane_info.get_mut(&pane_id) {
            pane_state.cwd = cwd;
            let tab_index = pane_state.tab_index;

            if self.focused_panes.get(&tab_index) == Some(&pane_id) {
                self.update_tab_name(tab_index, &pane_id);
            }
        } else {
            // Pane not yet known (new tab or session restore) — buffer for handle_pane_update
            self.pending_cwds.insert(pane_id, cwd);
        }
    }

    fn handle_tab_update(&mut self, tabs: &[TabInfo]) {
        let active_positions: HashSet<usize> = tabs.iter().map(|t| t.position).collect();

        self.current_tab_names
            .retain(|k, _| active_positions.contains(k));
        self.focused_panes
            .retain(|k, _| active_positions.contains(k));

        // Invalidate cache when actual tab name differs from expected
        // (another plugin like zellij-tab-status may have overwritten our rename)
        // Tolerate prefixes added by other plugins (e.g. "◉ dotfiles" matches "dotfiles")
        for tab in tabs {
            if let Some(expected) = self.current_tab_names.get(&tab.position) {
                if tab.name != *expected && !tab.name.ends_with(expected.as_str()) {
                    eprintln!(
                        "[cwd-plugin] tab {} name mismatch: expected \"{}\", actual \"{}\" — invalidating",
                        tab.position, expected, tab.name
                    );
                    self.current_tab_names.remove(&tab.position);
                }
            }
        }

        // Re-trigger rename for tabs whose name was overwritten
        for tab in tabs {
            if !self.current_tab_names.contains_key(&tab.position) {
                if let Some(&pane_id) = self.focused_panes.get(&tab.position) {
                    self.update_tab_name(tab.position, &pane_id);
                }
            }
        }

        // Clean up pane_info for panes in tabs that no longer exist
        self.pane_info
            .retain(|_, state| active_positions.contains(&state.tab_index));
    }

    fn handle_pane_update(&mut self, manifest: PaneManifest) {
        // Track which panes we see in this update
        let mut seen_panes: HashSet<PaneId> = HashSet::new();
        // Collect tabs that need a rename after processing (from pending CWDs or new focused panes)
        let mut tabs_to_rename: HashSet<usize> = HashSet::new();

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

                // Apply any pending CWD that arrived before this pane was known
                if let Some(pending_cwd) = self.pending_cwds.remove(&pane_id) {
                    eprintln!("[cwd-plugin] draining pending CWD for pane {:?}: {}", pane_id, pending_cwd.display());
                    pane_state.cwd = pending_cwd;
                    if pane.is_focused && !pane.is_plugin {
                        tabs_to_rename.insert(*tab_index);
                    }
                }

                // For focused terminal panes with no CWD, actively fetch it
                // (CwdChanged is not guaranteed for the initial CWD of a new pane)
                if pane.is_focused && !pane.is_plugin && pane_state.cwd.as_os_str().is_empty() {
                    eprintln!("[cwd-plugin] focused pane {:?} has no CWD, fetching...", pane_id);
                    match get_pane_cwd(pane_id) {
                        Ok(cwd) if !cwd.as_os_str().is_empty() => {
                            eprintln!("[cwd-plugin] get_pane_cwd ok: {}", cwd.display());
                            pane_state.cwd = cwd;
                        }
                        Ok(_) => {
                            eprintln!("[cwd-plugin] get_pane_cwd returned empty");
                        }
                        Err(e) => {
                            eprintln!("[cwd-plugin] get_pane_cwd error: {:?}", e);
                        }
                    }
                }

                if pane.is_focused && !pane.is_plugin {
                    let prev_focused = self.focused_panes.insert(*tab_index, pane_id);

                    if prev_focused != Some(pane_id) {
                        tabs_to_rename.insert(*tab_index);
                    } else if !pane_state.cwd.as_os_str().is_empty()
                        && !self.current_tab_names.contains_key(tab_index)
                    {
                        // Focused pane with a CWD but tab never renamed (session restore)
                        tabs_to_rename.insert(*tab_index);
                    }
                }
            }
        }

        // Remove panes that are no longer present
        self.pane_info.retain(|id, _| seen_panes.contains(id));

        // Now rename all tabs that need it
        if !tabs_to_rename.is_empty() {
            eprintln!("[cwd-plugin] tabs marked for rename: {:?}", tabs_to_rename);
        }
        for tab_index in tabs_to_rename {
            if let Some(&pane_id) = self.focused_panes.get(&tab_index) {
                self.update_tab_name(tab_index, &pane_id);
            }
        }
    }

    fn handle_run_command_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        context: BTreeMap<String, String>,
    ) {
        let Some(cwd_str) = context.get("cwd") else {
            return;
        };
        let cwd = PathBuf::from(cwd_str);

        let git_root = if exit_code == Some(0) {
            let root = String::from_utf8_lossy(&stdout).trim().to_string();
            if root.is_empty() {
                None
            } else {
                Some(PathBuf::from(root))
            }
        } else {
            None
        };

        self.git_root_cache.insert(cwd.clone(), git_root);

        if let Some(tab_indices) = self.pending_git_lookups.remove(&cwd) {
            for tab_index in tab_indices {
                if let Some(&pane_id) = self.focused_panes.get(&tab_index) {
                    self.update_tab_name(tab_index, &pane_id);
                }
            }
        }
    }

    /// Look up the git root for a path. Returns:
    /// - Some(Some(root)) if known to be in a git repo
    /// - Some(None) if known to NOT be in a git repo
    /// - None if lookup is pending (async command fired)
    fn resolve_git_root(&mut self, cwd: &Path, tab_index: usize) -> Option<Option<PathBuf>> {
        if let Some(cached) = self.git_root_cache.get(cwd) {
            return Some(cached.clone());
        }

        // Check if an ancestor's git root covers this path (pick the longest/most specific match)
        let mut best_match: Option<PathBuf> = None;
        for cached_root in self.git_root_cache.values().flatten() {
            if cwd.starts_with(cached_root) {
                if best_match.as_ref().is_none_or(|prev| cached_root.as_os_str().len() > prev.as_os_str().len()) {
                    best_match = Some(cached_root.clone());
                }
            }
        }
        if let Some(root) = best_match {
            self.git_root_cache.insert(cwd.to_path_buf(), Some(root.clone()));
            return Some(Some(root));
        }

        // Cache miss: register this tab as waiting
        self.pending_git_lookups
            .entry(cwd.to_path_buf())
            .or_default()
            .push(tab_index);

        // Only fire the async git command if we're the first waiter
        if self.pending_git_lookups[cwd].len() == 1 {
            let mut context = BTreeMap::new();
            context.insert("cwd".to_string(), cwd.to_string_lossy().to_string());
            run_command_with_env_variables_and_cwd(
                &["git", "rev-parse", "--show-toplevel"],
                BTreeMap::new(),
                cwd.to_path_buf(),
                context,
            );
        }

        None
    }

    fn rebase_on_git_root(&mut self, path: &Path, tab_index: usize) -> PathBuf {
        if !self.config.git_root {
            return path.to_path_buf();
        }

        let git_root = match self.resolve_git_root(path, tab_index) {
            Some(root) => root,
            None => return path.to_path_buf(),
        };

        let Some(git_root) = git_root else {
            return path.to_path_buf();
        };

        let root_name = git_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        path.strip_prefix(&git_root)
            .map(|relative| {
                if relative.as_os_str().is_empty() {
                    PathBuf::from(root_name)
                } else {
                    PathBuf::from(root_name).join(relative)
                }
            })
            .unwrap_or_else(|_| path.to_path_buf())
    }

    fn update_tab_name(&mut self, tab_index: usize, pane_id: &PaneId) {
        let (cwd, title) = if let Some(pane_state) = self.pane_info.get(pane_id) {
            if self.should_exclude(&pane_state.cwd) {
                return;
            }
            (pane_state.cwd.clone(), pane_state.title.clone())
        } else {
            return;
        };

        let name = match self.config.source {
            Source::Process => {
                if !title.is_empty() && !Self::is_shell_name(&title) {
                    title
                } else {
                    let rebased = self.rebase_on_git_root(&cwd, tab_index);
                    if rebased != cwd {
                        rebased.display().to_string()
                    } else {
                        self.format_path(&rebased)
                    }
                }
            }
            Source::Cwd => {
                let rebased = self.rebase_on_git_root(&cwd, tab_index);
                if rebased != cwd {
                    rebased.display().to_string()
                } else {
                    self.format_path(&rebased)
                }
            }
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

        // Use PathBuf's FromIterator to handle path separators correctly
        components[start..]
            .iter()
            .collect::<PathBuf>()
            .to_string_lossy()
            .to_string()
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
            eprintln!("[cwd-plugin] renaming tab {} -> \"{}\"", tab_position, new_name);
            // rename_tab expects 1-based position, PaneManifest keys are 0-based
            rename_tab(tab_position as u32 + 1, new_name);
            self.current_tab_names
                .insert(tab_position, new_name.to_string());
        } else if !should_rename {
            eprintln!("[cwd-plugin] tab {} skip (already \"{}\")", tab_position, new_name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // === Helper functions ===

    fn make_config(entries: &[(&str, &str)]) -> Config {
        let mut map = BTreeMap::new();
        for (k, v) in entries {
            map.insert(k.to_string(), v.to_string());
        }
        Config::from_configuration(&map)
    }

    fn make_state_with_config(config: Config) -> State {
        State {
            config,
            ..Default::default()
        }
    }

    // === Config::from_configuration tests ===

    #[test]
    fn test_config_default() {
        let config = make_config(&[]);
        assert_eq!(config.source, Source::Cwd);
        assert_eq!(config.format, Format::Basename);
        assert!(!config.git_root);
        assert_eq!(config.max_length, 0);
        assert_eq!(config.truncate_side, TruncateSide::Right);
        assert!(config.prefix.is_empty());
        assert!(config.suffix.is_empty());
        assert!(config.excludes.is_empty());
    }

    #[test]
    fn test_config_git_root_enabled() {
        let config = make_config(&[("git_root", "true")]);
        assert!(config.git_root);
    }

    #[test]
    fn test_config_git_root_disabled() {
        let config = make_config(&[("git_root", "false")]);
        assert!(!config.git_root);
    }

    #[test]
    fn test_config_source_process() {
        let config = make_config(&[("source", "process")]);
        assert_eq!(config.source, Source::Process);
    }

    #[test]
    fn test_config_format_full() {
        let config = make_config(&[("format", "full")]);
        assert_eq!(config.format, Format::Full);
    }

    #[test]
    fn test_config_format_tilde() {
        let config = make_config(&[("format", "tilde")]);
        assert_eq!(config.format, Format::Tilde);
    }

    #[test]
    fn test_config_format_segments() {
        let config = make_config(&[("format", "segments:3")]);
        assert_eq!(config.format, Format::Segments(3));
    }

    #[test]
    fn test_config_format_segments_invalid() {
        let config = make_config(&[("format", "segments:abc")]);
        assert_eq!(config.format, Format::Segments(1)); // default to 1
    }

    #[test]
    fn test_config_max_length() {
        let config = make_config(&[("max_length", "25")]);
        assert_eq!(config.max_length, 25);
    }

    #[test]
    fn test_config_truncate_left() {
        let config = make_config(&[("truncate_side", "left")]);
        assert_eq!(config.truncate_side, TruncateSide::Left);
    }

    #[test]
    fn test_config_prefix_suffix() {
        let config = make_config(&[("prefix", "["), ("suffix", "]")]);
        assert_eq!(config.prefix, "[");
        assert_eq!(config.suffix, "]");
    }

    #[test]
    fn test_config_excludes_parsing() {
        let config = make_config(&[("exclude", "/tmp:/var:/proc")]);
        assert_eq!(config.excludes.len(), 3);
        assert_eq!(config.excludes[0], PathBuf::from("/tmp"));
        assert_eq!(config.excludes[1], PathBuf::from("/var"));
        assert_eq!(config.excludes[2], PathBuf::from("/proc"));
    }

    #[test]
    fn test_config_home_dir_explicit() {
        let config = make_config(&[("home_dir", "/home/test")]);
        assert_eq!(config.home_dir, Some(PathBuf::from("/home/test")));
    }

    // === is_shell_name tests ===

    #[test]
    fn test_is_shell_name_lowercase() {
        assert!(State::is_shell_name("bash"));
        assert!(State::is_shell_name("zsh"));
        assert!(State::is_shell_name("fish"));
        assert!(State::is_shell_name("sh"));
        assert!(State::is_shell_name("nu"));
    }

    #[test]
    fn test_is_shell_name_uppercase() {
        assert!(State::is_shell_name("BASH"));
        assert!(State::is_shell_name("ZSH"));
        assert!(State::is_shell_name("FISH"));
    }

    #[test]
    fn test_is_shell_name_mixed_case() {
        assert!(State::is_shell_name("Bash"));
        assert!(State::is_shell_name("ZsH"));
    }

    #[test]
    fn test_is_shell_name_not_shell() {
        assert!(!State::is_shell_name("vim"));
        assert!(!State::is_shell_name("htop"));
        assert!(!State::is_shell_name("cargo"));
        assert!(!State::is_shell_name(""));
    }

    // === truncate_if_needed tests ===

    #[test]
    fn test_truncate_disabled() {
        let state = make_state_with_config(make_config(&[])); // max_length = 0
        assert_eq!(state.truncate_if_needed("very long string"), "very long string");
    }

    #[test]
    fn test_truncate_not_needed() {
        let state = make_state_with_config(make_config(&[("max_length", "20")]));
        assert_eq!(state.truncate_if_needed("short"), "short");
    }

    #[test]
    fn test_truncate_right() {
        let state = make_state_with_config(make_config(&[("max_length", "10")]));
        assert_eq!(state.truncate_if_needed("this is a long string"), "this is...");
    }

    #[test]
    fn test_truncate_left() {
        let state = make_state_with_config(make_config(&[
            ("max_length", "10"),
            ("truncate_side", "left"),
        ]));
        // "this is a long string" = 21 chars, keep last 7 = " string"
        assert_eq!(state.truncate_if_needed("this is a long string"), "... string");
    }

    #[test]
    fn test_truncate_utf8_chars() {
        // "café" is 4 characters but 5 bytes
        let state = make_state_with_config(make_config(&[("max_length", "7")]));
        let result = state.truncate_if_needed("café world");
        // Should truncate at character boundary, not byte boundary
        assert_eq!(result, "café...");
    }

    #[test]
    fn test_truncate_emoji() {
        // Emojis are multi-byte
        let state = make_state_with_config(make_config(&[("max_length", "6")]));
        let result = state.truncate_if_needed("🚀🎉🔥 test");
        assert_eq!(result, "🚀🎉🔥...");
    }

    // === last_n_segments tests ===

    #[test]
    fn test_last_n_segments_basic() {
        let state = make_state_with_config(make_config(&[]));
        let path = Path::new("/home/user/projects/myapp");
        assert_eq!(state.last_n_segments(path, 2), "projects/myapp");
    }

    #[test]
    fn test_last_n_segments_one() {
        let state = make_state_with_config(make_config(&[]));
        let path = Path::new("/home/user/projects/myapp");
        assert_eq!(state.last_n_segments(path, 1), "myapp");
    }

    #[test]
    fn test_last_n_segments_more_than_available() {
        let state = make_state_with_config(make_config(&[]));
        let path = Path::new("/home/user");
        assert_eq!(state.last_n_segments(path, 10), "/home/user");
    }

    #[test]
    fn test_last_n_segments_three() {
        let state = make_state_with_config(make_config(&[]));
        let path = Path::new("/a/b/c/d/e");
        assert_eq!(state.last_n_segments(path, 3), "c/d/e");
    }

    // === replace_home_with_tilde tests ===

    #[test]
    fn test_replace_home_with_tilde_subdir() {
        let state = make_state_with_config(make_config(&[("home_dir", "/home/user")]));
        let path = Path::new("/home/user/projects/myapp");
        assert_eq!(state.replace_home_with_tilde(path), "~/projects/myapp");
    }

    #[test]
    fn test_replace_home_exact_match() {
        let state = make_state_with_config(make_config(&[("home_dir", "/home/user")]));
        let path = Path::new("/home/user");
        assert_eq!(state.replace_home_with_tilde(path), "~");
    }

    #[test]
    fn test_replace_home_no_match() {
        let state = make_state_with_config(make_config(&[("home_dir", "/home/user")]));
        let path = Path::new("/var/log/syslog");
        assert_eq!(state.replace_home_with_tilde(path), "/var/log/syslog");
    }

    #[test]
    fn test_replace_home_no_home_configured() {
        let mut config = make_config(&[]);
        config.home_dir = None;
        let state = make_state_with_config(config);
        let path = Path::new("/home/user/test");
        assert_eq!(state.replace_home_with_tilde(path), "/home/user/test");
    }

    // === should_exclude tests ===

    #[test]
    fn test_should_exclude_match() {
        let state = make_state_with_config(make_config(&[("exclude", "/tmp:/var")]));
        assert!(state.should_exclude(Path::new("/tmp")));
        assert!(state.should_exclude(Path::new("/tmp/foo")));
        assert!(state.should_exclude(Path::new("/var/log")));
    }

    #[test]
    fn test_should_exclude_no_match() {
        let state = make_state_with_config(make_config(&[("exclude", "/tmp:/var")]));
        assert!(!state.should_exclude(Path::new("/home/user")));
        assert!(!state.should_exclude(Path::new("/usr/local")));
    }

    #[test]
    fn test_should_exclude_empty() {
        let state = make_state_with_config(make_config(&[]));
        assert!(!state.should_exclude(Path::new("/tmp")));
    }

    // === format_path tests ===

    #[test]
    fn test_format_path_basename() {
        let state = make_state_with_config(make_config(&[("format", "basename")]));
        let path = Path::new("/home/user/projects/myapp");
        assert_eq!(state.format_path(path), "myapp");
    }

    #[test]
    fn test_format_path_full() {
        let state = make_state_with_config(make_config(&[("format", "full")]));
        let path = Path::new("/home/user/projects/myapp");
        assert_eq!(state.format_path(path), "/home/user/projects/myapp");
    }

    #[test]
    fn test_format_path_tilde() {
        let state = make_state_with_config(make_config(&[
            ("format", "tilde"),
            ("home_dir", "/home/user"),
        ]));
        let path = Path::new("/home/user/projects/myapp");
        assert_eq!(state.format_path(path), "~/projects/myapp");
    }

    #[test]
    fn test_format_path_segments() {
        let state = make_state_with_config(make_config(&[("format", "segments:2")]));
        let path = Path::new("/home/user/projects/myapp");
        assert_eq!(state.format_path(path), "projects/myapp");
    }

    #[test]
    fn test_format_path_empty() {
        let state = make_state_with_config(make_config(&[]));
        let path = Path::new("");
        assert_eq!(state.format_path(path), "");
    }

    #[test]
    fn test_format_path_root() {
        let state = make_state_with_config(make_config(&[("format", "basename")]));
        let path = Path::new("/");
        // Root has no file_name, should return "~" as fallback
        assert_eq!(state.format_path(path), "~");
    }

    // === rebase_on_git_root tests ===

    fn make_state_with_git_cache(
        config: Config,
        cache: Vec<(PathBuf, Option<PathBuf>)>,
    ) -> State {
        let mut state = make_state_with_config(config);
        for (path, root) in cache {
            state.git_root_cache.insert(path, root);
        }
        state
    }

    #[test]
    fn test_rebase_subdir_in_git_repo() {
        let mut state = make_state_with_git_cache(
            make_config(&[("git_root", "true")]),
            vec![(
                PathBuf::from("/home/user/dev/proj/src/lib"),
                Some(PathBuf::from("/home/user/dev/proj")),
            )],
        );
        let result = state.rebase_on_git_root(Path::new("/home/user/dev/proj/src/lib"), 0);
        assert_eq!(result, PathBuf::from("proj/src/lib"));
    }

    #[test]
    fn test_rebase_at_git_root() {
        let mut state = make_state_with_git_cache(
            make_config(&[("git_root", "true")]),
            vec![(
                PathBuf::from("/home/user/dev/proj"),
                Some(PathBuf::from("/home/user/dev/proj")),
            )],
        );
        let result = state.rebase_on_git_root(Path::new("/home/user/dev/proj"), 0);
        assert_eq!(result, PathBuf::from("proj"));
    }

    #[test]
    fn test_rebase_not_a_git_repo() {
        let mut state = make_state_with_git_cache(
            make_config(&[("git_root", "true")]),
            vec![(PathBuf::from("/tmp/something"), None)],
        );
        let result = state.rebase_on_git_root(Path::new("/tmp/something"), 0);
        assert_eq!(result, PathBuf::from("/tmp/something"));
    }

    #[test]
    fn test_rebase_disabled() {
        let mut state = make_state_with_git_cache(
            make_config(&[]),
            vec![(
                PathBuf::from("/home/user/dev/proj/src"),
                Some(PathBuf::from("/home/user/dev/proj")),
            )],
        );
        let result = state.rebase_on_git_root(Path::new("/home/user/dev/proj/src"), 0);
        assert_eq!(result, PathBuf::from("/home/user/dev/proj/src"));
    }

    #[test]
    fn test_rebase_ancestor_cache_hit() {
        // Cache has root for /home/user/dev/proj, but we query a subdir
        let mut state = make_state_with_git_cache(
            make_config(&[("git_root", "true")]),
            vec![(
                PathBuf::from("/home/user/dev/proj"),
                Some(PathBuf::from("/home/user/dev/proj")),
            )],
        );
        let result = state.rebase_on_git_root(Path::new("/home/user/dev/proj/src/main.rs"), 0);
        assert_eq!(result, PathBuf::from("proj/src/main.rs"));
    }

    #[test]
    fn test_rebase_with_format_full() {
        let mut state = make_state_with_git_cache(
            make_config(&[("git_root", "true"), ("format", "full")]),
            vec![(
                PathBuf::from("/home/user/dev/proj/src"),
                Some(PathBuf::from("/home/user/dev/proj")),
            )],
        );
        let rebased = state.rebase_on_git_root(Path::new("/home/user/dev/proj/src"), 0);
        assert_eq!(state.format_path(&rebased), "proj/src");
    }

    #[test]
    fn test_rebase_with_format_segments() {
        let mut state = make_state_with_git_cache(
            make_config(&[("git_root", "true"), ("format", "segments:2")]),
            vec![(
                PathBuf::from("/home/user/dev/proj/src/lib"),
                Some(PathBuf::from("/home/user/dev/proj")),
            )],
        );
        let rebased = state.rebase_on_git_root(Path::new("/home/user/dev/proj/src/lib"), 0);
        assert_eq!(state.format_path(&rebased), "src/lib");
    }

}
