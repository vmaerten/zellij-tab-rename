mod config;
mod decorations;
mod format;
mod rename;

use std::collections::{BTreeMap, HashMap};
use zellij_tile::prelude::*;

#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        eprintln!($($arg)*);
    };
}

use config::{Config, Source};
use decorations::DecorationState;
use format::compose_tab_name;
use rename::RenameState;

#[derive(Default)]
struct State {
    config: Config,
    rename: RenameState,
    decorations: DecorationState,
    got_permissions: bool,
    buffered_events: Vec<Event>,
    current_tab_names: HashMap<usize, String>,
    active_tab: Option<usize>,
}

register_plugin!(State);

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
            PermissionType::ReadCliPipes,
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
                debug_log!("[cwd-plugin] PermissionRequestResult: {:?}", result);
                if result == PermissionStatus::Granted {
                    self.got_permissions = true;
                    let buffered = std::mem::take(&mut self.buffered_events);
                    debug_log!("[cwd-plugin] replaying {} buffered events", buffered.len());
                    for ev in buffered {
                        self.process_event(ev);
                    }
                }
            }
            event => {
                if self.got_permissions {
                    self.process_event(event);
                } else {
                    debug_log!("[cwd-plugin] permissions pending, buffering event");
                    self.buffered_events.push(event);
                }
            }
        }
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        debug_log!(
            "[cwd-plugin] pipe: name={}, args={:?}",
            pipe_message.name, pipe_message.args
        );
        let cli_pipe_id = match &pipe_message.source {
            PipeSource::Cli(id) => Some(id.clone()),
            _ => None,
        };
        let handled = self.handle_pipe(pipe_message);
        if let Some(ref pipe_id) = cli_pipe_id {
            unblock_cli_pipe_input(pipe_id);
        }
        handled
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    fn process_event(&mut self, event: Event) {
        match event {
            Event::CwdChanged(pane_id, cwd, _) => {
                debug_log!(
                    "[cwd-plugin] event: CwdChanged pane={:?} cwd={}",
                    pane_id,
                    cwd.display()
                );
                self.handle_cwd_changed(pane_id, cwd);
            }
            Event::TabUpdate(tabs) => {
                self.handle_tab_update(&tabs);
            }
            Event::PaneUpdate(manifest) => {
                self.handle_pane_update(manifest);
            }
            Event::RunCommandResult(exit_code, stdout, _stderr, context) => {
                debug_log!("[cwd-plugin] event: RunCommandResult exit={:?}", exit_code);
                self.handle_run_command_result(exit_code, stdout, context);
            }
            _ => {}
        }
    }

    fn update_tab_name(&mut self, tab_index: usize, pane_id: &PaneId) {
        let (cwd, title) = if let Some(pane_state) = self.rename.pane_info.get(pane_id) {
            if self.should_exclude(&pane_state.cwd) {
                return;
            }
            (pane_state.cwd.clone(), pane_state.title.clone())
        } else {
            return;
        };

        let base_path = self.rebase_on_git_root(&cwd, tab_index);
        let name = match self.config.source {
            Source::Process if !title.is_empty() && !Self::is_shell_name(&title) => title,
            _ => self.format_path(&base_path),
        };

        if name.is_empty() {
            return;
        }

        let pane_count_suffix = if !self.config.pane_count.is_empty() {
            let count = self.rename.pane_counts.get(&tab_index).copied().unwrap_or(1);
            if count >= self.config.pane_count_min {
                self.config
                    .pane_count
                    .replace("{count}", &count.to_string())
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let deco = self.decorations.tab_decorations.get(&tab_index);
        let final_name = compose_tab_name(
            &name,
            &self.config.prefix,
            &self.config.suffix,
            &pane_count_suffix,
            deco,
            self.config.max_length,
            &self.config.truncate_side,
        );
        self.rename_tab_if_needed(tab_index, &final_name);
    }

    fn rename_tab_if_needed(&mut self, tab_position: usize, new_name: &str) {
        let should_rename = self
            .current_tab_names
            .get(&tab_position)
            .is_none_or(|current| current != new_name);

        if should_rename && !new_name.is_empty() {
            debug_log!(
                "[cwd-plugin] renaming tab {} -> \"{}\"",
                tab_position, new_name
            );
            rename_tab(tab_position as u32 + 1, new_name);
            self.current_tab_names
                .insert(tab_position, new_name.to_string());
        } else if !should_rename {
            debug_log!(
                "[cwd-plugin] tab {} skip (already \"{}\")",
                tab_position, new_name
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    pub fn make_config(entries: &[(&str, &str)]) -> Config {
        let mut map = BTreeMap::new();
        for (k, v) in entries {
            map.insert(k.to_string(), v.to_string());
        }
        Config::from_configuration(&map)
    }

    pub fn make_state_with_config(config: Config) -> State {
        State {
            config,
            ..Default::default()
        }
    }
}
