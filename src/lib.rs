use std::collections::BTreeMap;
use std::path::PathBuf;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    // Map: tab_index -> focused pane id in that tab
    focused_panes: BTreeMap<usize, PaneId>,
    // Map: pane_id -> (tab_index, cwd)
    pane_info: BTreeMap<PaneId, (usize, PathBuf)>,
    // Current tab names to avoid unnecessary renames
    current_tab_names: BTreeMap<usize, String>,
    // Have we received permissions?
    got_permissions: bool,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
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
        false // Background plugin - no render needed
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        // Background plugin - nothing to render
    }
}

impl State {
    fn handle_cwd_changed(&mut self, pane_id: PaneId, cwd: PathBuf) {
        // Update the cwd for this pane
        if let Some((tab_index, _)) = self.pane_info.get(&pane_id) {
            let tab_index = *tab_index;
            self.pane_info.insert(pane_id, (tab_index, cwd.clone()));

            // If this pane is the focused pane for its tab, rename the tab
            if self.focused_panes.get(&tab_index) == Some(&pane_id) {
                let new_name = Self::extract_dirname(&cwd);
                self.rename_tab_if_needed(tab_index, &new_name);
            }
        }
    }

    fn handle_tab_update(&mut self, tabs: &[TabInfo]) {
        // Clean up tabs that no longer exist
        let active_positions: Vec<usize> = tabs.iter().map(|t| t.position).collect();
        self.current_tab_names.retain(|k, _| active_positions.contains(k));
        self.focused_panes.retain(|k, _| active_positions.contains(k));
    }

    fn handle_pane_update(&mut self, manifest: PaneManifest) {
        // Update our knowledge of which panes are in which tabs and which are focused
        for (tab_index, panes) in manifest.panes {
            for pane in &panes {
                let pane_id = PaneId::Terminal(pane.id);

                // Track this pane's tab
                if let Some((_, cwd)) = self.pane_info.get(&pane_id) {
                    self.pane_info.insert(pane_id, (tab_index, cwd.clone()));
                } else {
                    self.pane_info.insert(pane_id, (tab_index, PathBuf::new()));
                }

                // Track focused pane per tab
                if pane.is_focused {
                    let prev_focused = self.focused_panes.insert(tab_index, pane_id);

                    // If focused pane changed, update tab name based on the new focused pane's cwd
                    if prev_focused != Some(pane_id) {
                        if let Some((_, cwd)) = self.pane_info.get(&pane_id) {
                            if !cwd.as_os_str().is_empty() {
                                let new_name = Self::extract_dirname(cwd);
                                self.rename_tab_if_needed(tab_index, &new_name);
                            }
                        }
                    }
                }
            }
        }
    }

    fn extract_dirname(path: &PathBuf) -> String {
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("~")
            .to_string()
    }

    fn rename_tab_if_needed(&mut self, tab_position: usize, new_name: &str) {
        let should_rename = match self.current_tab_names.get(&tab_position) {
            Some(current) => current != new_name,
            None => true,
        };

        if should_rename && !new_name.is_empty() {
            rename_tab(tab_position as u32, new_name);
            self.current_tab_names
                .insert(tab_position, new_name.to_string());
        }
    }
}
