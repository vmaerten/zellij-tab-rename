use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use zellij_tile::prelude::*;

use crate::config::Format;

const SHELL_NAMES: &[&str] = &[
    "bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "csh", "nu", "nushell",
];

#[derive(Default)]
pub(crate) struct PaneState {
    pub tab_index: usize,
    pub cwd: PathBuf,
    pub title: String,
}

#[derive(Default)]
pub(crate) struct RenameState {
    pub focused_panes: HashMap<usize, PaneId>,
    pub pane_info: HashMap<PaneId, PaneState>,
    /// CWDs received before PaneUpdate (race condition on new tab / session restore)
    pub pending_cwds: HashMap<PaneId, PathBuf>,
    /// cwd -> Some(git_root) or None (not a git repo)
    pub git_root_cache: HashMap<PathBuf, Option<PathBuf>>,
    /// cwds with pending git lookups -> tab indices waiting for the result
    pub pending_git_lookups: HashMap<PathBuf, Vec<usize>>,
}

impl super::State {
    pub(crate) fn handle_cwd_changed(&mut self, pane_id: PaneId, cwd: PathBuf) {
        if let Some(pane_state) = self.rename.pane_info.get_mut(&pane_id) {
            pane_state.cwd = cwd;
            let tab_index = pane_state.tab_index;

            if self.rename.focused_panes.get(&tab_index) == Some(&pane_id) {
                self.update_tab_name(tab_index, &pane_id);
            }
        } else {
            self.rename.pending_cwds.insert(pane_id, cwd);
        }
    }

    pub(crate) fn handle_tab_update(&mut self, tabs: &[TabInfo]) {
        let active_positions: HashSet<usize> = tabs.iter().map(|t| t.position).collect();

        self.active_tab = tabs.iter().find(|t| t.active).map(|t| t.position);

        self.current_tab_names
            .retain(|k, _| active_positions.contains(k));
        self.rename
            .focused_panes
            .retain(|k, _| active_positions.contains(k));
        self.decorations
            .tab_decorations
            .retain(|k, _| active_positions.contains(k));
        self.decorations
            .tab_decoration_source
            .retain(|k, _| active_positions.contains(k));

        // Invalidate cache when actual tab name differs from expected
        for tab in tabs {
            if let Some(expected) = self.current_tab_names.get(&tab.position) {
                if tab.name != *expected {
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
                if let Some(&pane_id) = self.rename.focused_panes.get(&tab.position) {
                    self.update_tab_name(tab.position, &pane_id);
                }
            }
        }

        // Clean up pane_info for panes in tabs that no longer exist
        self.rename
            .pane_info
            .retain(|_, state| active_positions.contains(&state.tab_index));
    }

    pub(crate) fn handle_pane_update(&mut self, manifest: PaneManifest) {
        let mut seen_panes: HashSet<PaneId> = HashSet::new();
        let mut tabs_to_rename: HashSet<usize> = HashSet::new();

        for (tab_index, panes) in &manifest.panes {
            for pane in panes {
                let pane_id = if pane.is_plugin {
                    PaneId::Plugin(pane.id)
                } else {
                    PaneId::Terminal(pane.id)
                };

                seen_panes.insert(pane_id);

                let pane_state = self.rename.pane_info.entry(pane_id).or_default();

                if pane_state.tab_index != *tab_index {
                    pane_state.tab_index = *tab_index;
                }
                if pane_state.title != pane.title {
                    pane_state.title.clone_from(&pane.title);
                }

                if let Some(pending_cwd) = self.rename.pending_cwds.remove(&pane_id) {
                    eprintln!(
                        "[cwd-plugin] draining pending CWD for pane {:?}: {}",
                        pane_id,
                        pending_cwd.display()
                    );
                    pane_state.cwd = pending_cwd;
                    if pane.is_focused && !pane.is_plugin {
                        tabs_to_rename.insert(*tab_index);
                    }
                }

                if pane.is_focused && !pane.is_plugin && pane_state.cwd.as_os_str().is_empty() {
                    eprintln!(
                        "[cwd-plugin] focused pane {:?} has no CWD, fetching...",
                        pane_id
                    );
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
                    let prev_focused =
                        self.rename.focused_panes.insert(*tab_index, pane_id);

                    if prev_focused != Some(pane_id) {
                        tabs_to_rename.insert(*tab_index);
                    } else if !pane_state.cwd.as_os_str().is_empty()
                        && !self.current_tab_names.contains_key(tab_index)
                    {
                        tabs_to_rename.insert(*tab_index);
                    }
                }
            }
        }

        self.rename.pane_info.retain(|id, _| seen_panes.contains(id));

        // Clean up decorations whose source pane is gone
        self.decorations
            .tab_decoration_source
            .retain(|tab_idx, source_pane| {
                if seen_panes.contains(source_pane) {
                    true
                } else {
                    self.decorations.tab_decorations.remove(tab_idx);
                    tabs_to_rename.insert(*tab_idx);
                    false
                }
            });

        if !tabs_to_rename.is_empty() {
            eprintln!("[cwd-plugin] tabs marked for rename: {:?}", tabs_to_rename);
        }
        for tab_index in tabs_to_rename {
            if let Some(&pane_id) = self.rename.focused_panes.get(&tab_index) {
                self.update_tab_name(tab_index, &pane_id);
            }
        }
    }

    pub(crate) fn handle_run_command_result(
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

        self.rename.git_root_cache.insert(cwd.clone(), git_root);

        if let Some(tab_indices) = self.rename.pending_git_lookups.remove(&cwd) {
            for tab_index in tab_indices {
                if let Some(&pane_id) = self.rename.focused_panes.get(&tab_index) {
                    self.update_tab_name(tab_index, &pane_id);
                }
            }
        }
    }

    /// Look up the git root for a path. Returns:
    /// - Some(Some(root)) if known to be in a git repo
    /// - Some(None) if known to NOT be in a git repo
    /// - None if lookup is pending (async command fired)
    pub(crate) fn resolve_git_root(
        &mut self,
        cwd: &Path,
        tab_index: usize,
    ) -> Option<Option<PathBuf>> {
        if let Some(cached) = self.rename.git_root_cache.get(cwd) {
            return Some(cached.clone());
        }

        // Check if an ancestor's git root covers this path
        let mut best_match: Option<PathBuf> = None;
        for cached_root in self.rename.git_root_cache.values().flatten() {
            if cwd.starts_with(cached_root) {
                if best_match
                    .as_ref()
                    .is_none_or(|prev| cached_root.as_os_str().len() > prev.as_os_str().len())
                {
                    best_match = Some(cached_root.clone());
                }
            }
        }
        if let Some(root) = best_match {
            self.rename
                .git_root_cache
                .insert(cwd.to_path_buf(), Some(root.clone()));
            return Some(Some(root));
        }

        // Cache miss: register this tab as waiting
        self.rename
            .pending_git_lookups
            .entry(cwd.to_path_buf())
            .or_default()
            .push(tab_index);

        // Only fire the async git command if we're the first waiter
        if self.rename.pending_git_lookups[cwd].len() == 1 {
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

    pub(crate) fn rebase_on_git_root(&mut self, path: &Path, tab_index: usize) -> PathBuf {
        if !self.config.git_root {
            return path.to_path_buf();
        }

        let Some(Some(git_root)) = self.resolve_git_root(path, tab_index) else {
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

    pub(crate) fn format_path(&self, path: &Path) -> String {
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
        components[start..]
            .iter()
            .collect::<PathBuf>()
            .to_string_lossy()
            .to_string()
    }

    pub(crate) fn should_exclude(&self, path: &Path) -> bool {
        self.config
            .excludes
            .iter()
            .any(|excl| path.starts_with(excl))
    }

    pub(crate) fn is_shell_name(name: &str) -> bool {
        SHELL_NAMES
            .iter()
            .any(|shell| shell.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{make_config, make_state_with_config};
    use std::path::{Path, PathBuf};

    use super::*;

    fn make_state_with_git_cache(
        config: crate::config::Config,
        cache: Vec<(PathBuf, Option<PathBuf>)>,
    ) -> super::super::State {
        let mut state = make_state_with_config(config);
        for (path, root) in cache {
            state.rename.git_root_cache.insert(path, root);
        }
        state
    }

    // === is_shell_name tests ===

    #[test]
    fn test_is_shell_name_lowercase() {
        assert!(super::super::State::is_shell_name("bash"));
        assert!(super::super::State::is_shell_name("zsh"));
        assert!(super::super::State::is_shell_name("fish"));
        assert!(super::super::State::is_shell_name("sh"));
        assert!(super::super::State::is_shell_name("nu"));
    }

    #[test]
    fn test_is_shell_name_uppercase() {
        assert!(super::super::State::is_shell_name("BASH"));
        assert!(super::super::State::is_shell_name("ZSH"));
        assert!(super::super::State::is_shell_name("FISH"));
    }

    #[test]
    fn test_is_shell_name_mixed_case() {
        assert!(super::super::State::is_shell_name("Bash"));
        assert!(super::super::State::is_shell_name("ZsH"));
    }

    #[test]
    fn test_is_shell_name_not_shell() {
        assert!(!super::super::State::is_shell_name("vim"));
        assert!(!super::super::State::is_shell_name("htop"));
        assert!(!super::super::State::is_shell_name("cargo"));
        assert!(!super::super::State::is_shell_name(""));
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
        assert_eq!(state.format_path(path), "~");
    }

    // === rebase_on_git_root tests ===

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
