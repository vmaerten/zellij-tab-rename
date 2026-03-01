use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Default, Debug, PartialEq)]
pub(crate) enum Source {
    #[default]
    Cwd,
    Process,
}

#[derive(Default, Debug, PartialEq)]
pub(crate) enum Format {
    #[default]
    Basename,
    Full,
    Tilde,
    Segments(usize),
}

#[derive(Default, Debug, PartialEq)]
pub(crate) enum TruncateSide {
    Left,
    #[default]
    Right,
}

#[derive(Default)]
pub(crate) struct Config {
    pub source: Source,
    pub format: Format,
    pub git_root: bool,
    pub max_length: usize, // 0 = unlimited
    pub truncate_side: TruncateSide,
    pub prefix: String,
    pub suffix: String,
    pub excludes: Vec<PathBuf>,
    pub home_dir: Option<PathBuf>,
}

impl Config {
    pub fn from_configuration(configuration: &BTreeMap<String, String>) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(entries: &[(&str, &str)]) -> Config {
        let mut map = BTreeMap::new();
        for (k, v) in entries {
            map.insert(k.to_string(), v.to_string());
        }
        Config::from_configuration(&map)
    }

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
        assert_eq!(config.format, Format::Segments(1));
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
}
