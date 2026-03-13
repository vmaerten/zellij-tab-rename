use crate::config::TruncateSide;
use crate::decorations::Decorations;

/// Compose a final tab name from a base CWD name, config prefix/suffix, and optional decorations.
/// Truncation applies to the base name only (before adding decorations), so icons don't get cut off.
pub(crate) fn compose_tab_name(
    base: &str,
    config_prefix: &str,
    config_suffix: &str,
    pane_count_suffix: &str,
    deco: Option<&Decorations>,
    max_length: usize,
    truncate_side: &TruncateSide,
) -> String {
    // Apply config prefix/suffix to base name
    let with_config = if config_prefix.is_empty() && config_suffix.is_empty() {
        base.to_string()
    } else {
        format!("{}{}{}", config_prefix, base, config_suffix)
    };

    // Truncate the config-wrapped name (before decoration and pane count)
    let truncated = truncate_name(&with_config, max_length, truncate_side);

    // Wrap with pane count suffix and decorations
    let deco_prefix = deco.map(|d| d.prefix.as_str()).unwrap_or("");
    let deco_suffix = deco.map(|d| d.suffix.as_str()).unwrap_or("");

    if deco_prefix.is_empty() && deco_suffix.is_empty() && pane_count_suffix.is_empty() {
        truncated
    } else {
        format!(
            "{}{}{}{}",
            deco_prefix, truncated, pane_count_suffix, deco_suffix
        )
    }
}

/// Truncate a string to max_length characters, adding "..." on the appropriate side.
pub(crate) fn truncate_name(s: &str, max_length: usize, truncate_side: &TruncateSide) -> String {
    let char_count = s.chars().count();
    if max_length == 0 || char_count <= max_length {
        return s.to_string();
    }
    let max = max_length.saturating_sub(3);
    match truncate_side {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_disabled() {
        assert_eq!(
            truncate_name("very long string", 0, &TruncateSide::Right),
            "very long string"
        );
    }

    #[test]
    fn test_truncate_not_needed() {
        assert_eq!(
            truncate_name("short", 20, &TruncateSide::Right),
            "short"
        );
    }

    #[test]
    fn test_truncate_right() {
        assert_eq!(
            truncate_name("this is a long string", 10, &TruncateSide::Right),
            "this is..."
        );
    }

    #[test]
    fn test_truncate_left() {
        assert_eq!(
            truncate_name("this is a long string", 10, &TruncateSide::Left),
            "... string"
        );
    }

    #[test]
    fn test_truncate_utf8_chars() {
        let result = truncate_name("café world", 7, &TruncateSide::Right);
        assert_eq!(result, "café...");
    }

    #[test]
    fn test_truncate_emoji() {
        let result = truncate_name("🚀🎉🔥 test", 6, &TruncateSide::Right);
        assert_eq!(result, "🚀🎉🔥...");
    }

    #[test]
    fn test_compose_plain() {
        let result = compose_tab_name("myapp", "", "", "", None, 0, &TruncateSide::Right);
        assert_eq!(result, "myapp");
    }

    #[test]
    fn test_compose_with_config_prefix_suffix() {
        let result = compose_tab_name("myapp", "[", "]", "", None, 0, &TruncateSide::Right);
        assert_eq!(result, "[myapp]");
    }

    #[test]
    fn test_compose_with_decorations() {
        let deco = Decorations {
            prefix: "🔨 ".to_string(),
            suffix: String::new(),
        };
        let result = compose_tab_name("myapp", "", "", "", Some(&deco), 0, &TruncateSide::Right);
        assert_eq!(result, "🔨 myapp");
    }

    #[test]
    fn test_compose_with_both() {
        let deco = Decorations {
            prefix: "🔨 ".to_string(),
            suffix: " ✓".to_string(),
        };
        let result = compose_tab_name("myapp", "[", "]", "", Some(&deco), 0, &TruncateSide::Right);
        assert_eq!(result, "🔨 [myapp] ✓");
    }

    #[test]
    fn test_compose_truncation_on_base_not_deco() {
        let deco = Decorations {
            prefix: "🔨 ".to_string(),
            suffix: String::new(),
        };
        let result = compose_tab_name("myapp", "[", "]", "", Some(&deco), 10, &TruncateSide::Right);
        assert_eq!(result, "🔨 [myapp]");
    }

    #[test]
    fn test_compose_truncation_triggers() {
        let deco = Decorations {
            prefix: "🔨 ".to_string(),
            suffix: String::new(),
        };
        let result = compose_tab_name(
            "very-long-name",
            "[",
            "]",
            "",
            Some(&deco),
            10,
            &TruncateSide::Right,
        );
        assert_eq!(result, "🔨 [very-l...");
    }

    #[test]
    fn test_compose_empty_decorations() {
        let deco = Decorations {
            prefix: String::new(),
            suffix: String::new(),
        };
        let result = compose_tab_name("myapp", "", "", "", Some(&deco), 0, &TruncateSide::Right);
        assert_eq!(result, "myapp");
    }

    #[test]
    fn test_compose_with_pane_count() {
        let result = compose_tab_name("myapp", "", "", " (3)", None, 0, &TruncateSide::Right);
        assert_eq!(result, "myapp (3)");
    }

    #[test]
    fn test_compose_pane_count_with_decorations() {
        let deco = Decorations {
            prefix: "🔨 ".to_string(),
            suffix: " ✓".to_string(),
        };
        let result =
            compose_tab_name("myapp", "", "", " (2)", Some(&deco), 0, &TruncateSide::Right);
        assert_eq!(result, "🔨 myapp (2) ✓");
    }

    #[test]
    fn test_compose_pane_count_after_truncation() {
        let result =
            compose_tab_name("very-long-name", "", "", " (3)", None, 10, &TruncateSide::Right);
        assert_eq!(result, "very-lo... (3)");
    }
}
