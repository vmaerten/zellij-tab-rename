use std::collections::HashMap;
use zellij_tile::prelude::*;

#[derive(Default, Clone, Debug, PartialEq)]
pub(crate) struct Decorations {
    pub prefix: String,
    pub suffix: String,
}

#[derive(Default)]
pub(crate) struct DecorationState {
    /// Per-tab decorations (prefix/suffix) set via pipe protocol
    pub tab_decorations: HashMap<usize, Decorations>,
    /// Which pane set the decoration on each tab (for cleanup when pane disappears)
    pub tab_decoration_source: HashMap<usize, PaneId>,
}

impl super::State {
    pub(crate) fn handle_pipe(&mut self, msg: PipeMessage) -> bool {
        let action = msg.name.as_str();
        match action {
            "set_prefix" | "set_suffix" | "clear" => {}
            _ => return false,
        }

        // Resolve target tab from args
        let tab_index = if let Some(pane_str) = msg.args.get("pane") {
            match pane_str.parse::<u32>() {
                Ok(id) => {
                    let pane_id = PaneId::Terminal(id);
                    self.rename
                        .pane_info
                        .get(&pane_id)
                        .map(|ps| (ps.tab_index, Some(pane_id)))
                }
                Err(_) => {
                    eprintln!("[cwd-plugin] pipe: invalid pane id \"{}\"", pane_str);
                    return true;
                }
            }
        } else if msg.args.get("tab").map(|s| s.as_str()) == Some("focused") {
            self.active_tab.map(|idx| (idx, None))
        } else if action == "clear" {
            self.clear_all_decorations();
            return true;
        } else {
            eprintln!(
                "[cwd-plugin] pipe: no pane or tab specified for \"{}\"",
                action
            );
            return true;
        };

        let Some((tab_idx, pane_id)) = tab_index else {
            eprintln!(
                "[cwd-plugin] pipe: could not resolve tab for args {:?}, known panes: {:?}",
                msg.args,
                self.rename.pane_info.keys().collect::<Vec<_>>()
            );
            return true;
        };

        // Extract payload, filtering out spurious stdin messages from hook metadata
        let value = match msg.payload {
            Some(ref p) if !p.is_empty() && !p.starts_with('{') => Some(p.clone()),
            _ => None,
        };

        match action {
            "set_prefix" => {
                let Some(v) = value else { return true };
                self.apply_decoration(tab_idx, pane_id, |d| d.prefix = v);
            }
            "set_suffix" => {
                let Some(v) = value else { return true };
                self.apply_decoration(tab_idx, pane_id, |d| d.suffix = v);
            }
            "clear" => {
                self.decorations.tab_decorations.remove(&tab_idx);
                self.decorations.tab_decoration_source.remove(&tab_idx);
            }
            _ => unreachable!(),
        }

        if let Some(&pane_id) = self.rename.focused_panes.get(&tab_idx) {
            self.update_tab_name(tab_idx, &pane_id);
        }
        true
    }

    fn apply_decoration(
        &mut self,
        tab_idx: usize,
        pane_id: Option<PaneId>,
        f: impl FnOnce(&mut Decorations),
    ) {
        let deco = self.decorations.tab_decorations.entry(tab_idx).or_default();
        f(deco);
        if let Some(pid) = pane_id {
            self.decorations.tab_decoration_source.insert(tab_idx, pid);
        }
    }

    pub(crate) fn clear_all_decorations(&mut self) {
        self.decorations.tab_decorations.clear();
        self.decorations.tab_decoration_source.clear();
        for (&tab_idx, &pane_id) in &self.rename.focused_panes.clone() {
            self.update_tab_name(tab_idx, &pane_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{make_config, make_state_with_config};
    use super::*;
    use crate::rename::PaneState;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn test_pipe_set_prefix() {
        let mut state = make_state_with_config(make_config(&[]));
        state.rename.pane_info.insert(
            PaneId::Terminal(42),
            PaneState {
                tab_index: 0,
                cwd: PathBuf::from("/home/user/myapp"),
                title: String::new(),
            },
        );
        state.rename.focused_panes.insert(0, PaneId::Terminal(42));

        let msg = PipeMessage {
            source: PipeSource::Cli("test".to_string()),
            name: "set_prefix".to_string(),
            payload: Some("🔨 ".to_string()),
            args: BTreeMap::from([("pane".to_string(), "42".to_string())]),
            is_private: false,
        };
        state.handle_pipe(msg);

        assert_eq!(
            state.decorations.tab_decorations.get(&0),
            Some(&Decorations {
                prefix: "🔨 ".to_string(),
                suffix: String::new()
            })
        );
        assert_eq!(
            state.decorations.tab_decoration_source.get(&0),
            Some(&PaneId::Terminal(42))
        );
    }

    #[test]
    fn test_pipe_set_suffix() {
        let mut state = make_state_with_config(make_config(&[]));
        state.rename.pane_info.insert(
            PaneId::Terminal(7),
            PaneState {
                tab_index: 2,
                cwd: PathBuf::from("/tmp"),
                title: String::new(),
            },
        );

        let msg = PipeMessage {
            source: PipeSource::Cli("test".to_string()),
            name: "set_suffix".to_string(),
            payload: Some(" ✓".to_string()),
            args: BTreeMap::from([("pane".to_string(), "7".to_string())]),
            is_private: false,
        };
        state.handle_pipe(msg);

        assert_eq!(
            state.decorations.tab_decorations.get(&2),
            Some(&Decorations {
                prefix: String::new(),
                suffix: " ✓".to_string()
            })
        );
    }

    #[test]
    fn test_pipe_clear_specific_pane() {
        let mut state = make_state_with_config(make_config(&[]));
        state.rename.pane_info.insert(
            PaneId::Terminal(42),
            PaneState {
                tab_index: 0,
                cwd: PathBuf::from("/tmp"),
                title: String::new(),
            },
        );
        state.decorations.tab_decorations.insert(
            0,
            Decorations {
                prefix: "🔨 ".to_string(),
                suffix: String::new(),
            },
        );
        state
            .decorations
            .tab_decoration_source
            .insert(0, PaneId::Terminal(42));

        let msg = PipeMessage {
            source: PipeSource::Cli("test".to_string()),
            name: "clear".to_string(),
            payload: None,
            args: BTreeMap::from([("pane".to_string(), "42".to_string())]),
            is_private: false,
        };
        state.handle_pipe(msg);

        assert!(state.decorations.tab_decorations.is_empty());
        assert!(state.decorations.tab_decoration_source.is_empty());
    }

    #[test]
    fn test_pipe_clear_all() {
        let mut state = make_state_with_config(make_config(&[]));
        state.decorations.tab_decorations.insert(
            0,
            Decorations {
                prefix: "🔨 ".to_string(),
                suffix: String::new(),
            },
        );
        state.decorations.tab_decorations.insert(
            1,
            Decorations {
                prefix: "⏳ ".to_string(),
                suffix: String::new(),
            },
        );
        state
            .decorations
            .tab_decoration_source
            .insert(0, PaneId::Terminal(1));
        state
            .decorations
            .tab_decoration_source
            .insert(1, PaneId::Terminal(2));

        let msg = PipeMessage {
            source: PipeSource::Cli("test".to_string()),
            name: "clear".to_string(),
            payload: None,
            args: BTreeMap::new(),
            is_private: false,
        };
        state.handle_pipe(msg);

        assert!(state.decorations.tab_decorations.is_empty());
        assert!(state.decorations.tab_decoration_source.is_empty());
    }

    #[test]
    fn test_pipe_focused_tab_fallback() {
        let mut state = make_state_with_config(make_config(&[]));
        state.active_tab = Some(1);

        let msg = PipeMessage {
            source: PipeSource::Cli("test".to_string()),
            name: "set_prefix".to_string(),
            payload: Some("⏳ ".to_string()),
            args: BTreeMap::from([("tab".to_string(), "focused".to_string())]),
            is_private: false,
        };
        state.handle_pipe(msg);

        assert_eq!(
            state.decorations.tab_decorations.get(&1),
            Some(&Decorations {
                prefix: "⏳ ".to_string(),
                suffix: String::new()
            })
        );
    }

    #[test]
    fn test_pipe_unknown_action_ignored() {
        let mut state = make_state_with_config(make_config(&[]));

        let msg = PipeMessage {
            source: PipeSource::Cli("test".to_string()),
            name: "unknown_action".to_string(),
            payload: None,
            args: BTreeMap::new(),
            is_private: false,
        };
        state.handle_pipe(msg);

        assert!(state.decorations.tab_decorations.is_empty());
    }

    #[test]
    fn test_pipe_invalid_pane_id() {
        let mut state = make_state_with_config(make_config(&[]));

        let msg = PipeMessage {
            source: PipeSource::Cli("test".to_string()),
            name: "set_prefix".to_string(),
            payload: Some("🔨 ".to_string()),
            args: BTreeMap::from([("pane".to_string(), "not_a_number".to_string())]),
            is_private: false,
        };
        state.handle_pipe(msg);

        assert!(state.decorations.tab_decorations.is_empty());
    }

    #[test]
    fn test_pipe_pane_not_found() {
        let mut state = make_state_with_config(make_config(&[]));
        let msg = PipeMessage {
            source: PipeSource::Cli("test".to_string()),
            name: "set_prefix".to_string(),
            payload: Some("🔨 ".to_string()),
            args: BTreeMap::from([("pane".to_string(), "999".to_string())]),
            is_private: false,
        };
        state.handle_pipe(msg);

        assert!(state.decorations.tab_decorations.is_empty());
    }
}
