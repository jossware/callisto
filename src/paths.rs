//! Where callisto reads and writes outside the vault.

use std::env;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// The environment, read once. Any part can be missing; callers decide
/// whether that matters.
#[derive(Debug, Clone, PartialEq)]
pub struct Env {
    pub home: Option<PathBuf>,
    /// `<config>/callisto`, holding `config.toml`.
    pub config: Option<PathBuf>,
    /// `<state>/callisto`, holding the error log and the sync lock.
    pub state: Option<PathBuf>,
}

impl Env {
    pub fn read() -> Env {
        Env::from_vars(
            env::var_os("HOME"),
            env::var_os("XDG_CONFIG_HOME"),
            env::var_os("XDG_STATE_HOME"),
        )
    }

    pub fn from_vars(
        home: Option<OsString>,
        xdg_config_home: Option<OsString>,
        xdg_state_home: Option<OsString>,
    ) -> Env {
        let home = home.filter(|h| !h.is_empty()).map(PathBuf::from);
        let dir = |xdg: Option<OsString>, fallback: &str| {
            // The XDG spec says to ignore a relative value.
            match xdg.map(PathBuf::from) {
                Some(xdg) if xdg.is_absolute() => Some(xdg.join("callisto")),
                _ => home.as_ref().map(|h| h.join(fallback).join("callisto")),
            }
        };
        let config = dir(xdg_config_home, ".config");
        let state = dir(xdg_state_home, ".local/state");
        Env {
            home,
            config,
            state,
        }
    }
}

pub fn config_file(config: &Path) -> PathBuf {
    config.join("config.toml")
}

pub fn sync_lock(state: &Path) -> PathBuf {
    state.join("sync.lock")
}

pub fn error_log(state: &Path) -> PathBuf {
    state.join("errors.jsonl")
}

/// Expands a leading `~` to `home`. Codex may run the hook without a
/// shell, and launchd and systemd never expand it. `None` means the path
/// needs a home and there is none.
pub fn expand_tilde(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let mut parts = path.components();
    match parts.next() {
        Some(Component::Normal(first)) if first == "~" => home.map(|h| h.join(parts.as_path())),
        _ => Some(path.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn env(home: Option<&str>, config: Option<&str>, state: Option<&str>) -> Env {
        Env::from_vars(
            home.map(Into::into),
            config.map(Into::into),
            state.map(Into::into),
        )
    }

    #[test]
    fn dirs_default_under_home() {
        assert_eq!(
            env(Some("/h"), None, None),
            Env {
                home: Some("/h".into()),
                config: Some("/h/.config/callisto".into()),
                state: Some("/h/.local/state/callisto".into()),
            }
        );
    }

    #[test]
    fn absolute_xdg_dirs_win() {
        let e = env(Some("/h"), Some("/c"), Some("/s"));
        assert_eq!(e.config, Some("/c/callisto".into()));
        assert_eq!(e.state, Some("/s/callisto".into()));
    }

    #[test]
    fn relative_xdg_dirs_are_ignored() {
        assert_eq!(
            env(Some("/h"), Some("c"), Some("s")),
            env(Some("/h"), None, None)
        );
    }

    #[test]
    fn empty_home_is_missing() {
        let none = Env {
            home: None,
            config: None,
            state: None,
        };
        assert_eq!(env(Some(""), None, None), none);
        assert_eq!(env(None, None, None), none);
    }

    #[test]
    fn xdg_dirs_survive_a_missing_home() {
        let e = env(None, Some("/c"), Some("/s"));
        assert_eq!(e.home, None);
        assert_eq!(e.config, Some("/c/callisto".into()));
        assert_eq!(e.state, Some("/s/callisto".into()));
    }

    #[test]
    fn state_files_are_named() {
        let state = Path::new("/s/callisto");
        assert_eq!(sync_lock(state), Path::new("/s/callisto/sync.lock"));
        assert_eq!(error_log(state), Path::new("/s/callisto/errors.jsonl"));
    }

    #[test]
    fn config_file_is_toml() {
        assert_eq!(
            config_file(Path::new("/c/callisto")),
            Path::new("/c/callisto/config.toml")
        );
    }

    #[test]
    fn expand_tilde_cases() {
        let home = Some(Path::new("/h"));
        let expand = |p: &str, home| expand_tilde(Path::new(p), home);
        assert_eq!(expand("~/org", home), Some("/h/org".into()));
        assert_eq!(expand("~", home), Some("/h".into()));
        assert_eq!(expand("/abs", home), Some("/abs".into()));
        assert_eq!(expand("rel", home), Some("rel".into()));
        assert_eq!(expand("~user/x", home), Some("~user/x".into()));
        assert_eq!(expand("~/org", None), None);
        assert_eq!(expand("/abs", None), Some("/abs".into()));
    }
}
