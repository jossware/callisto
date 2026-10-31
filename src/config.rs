//! The config file: which repos callisto syncs and where the vault is.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::paths::expand_tilde;

/// Paths are expanded and absolute.
#[derive(Debug, Default, PartialEq)]
pub struct Config {
    pub vault: Option<PathBuf>,
    pub repos: Vec<PathBuf>,
}

// A typo like `repo =` must fail loudly rather than empty the repo list.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    vault: Option<PathBuf>,
    #[serde(default)]
    repos: Vec<PathBuf>,
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    NoHome {
        path: PathBuf,
        entry: PathBuf,
    },
    Relative {
        path: PathBuf,
        entry: PathBuf,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, source } => write!(f, "{}: {source}", path.display()),
            ConfigError::Parse { path, source } => write!(f, "{}: {source}", path.display()),
            ConfigError::NoHome { path, entry } => write!(
                f,
                "{}: cannot expand {}: HOME is not set",
                path.display(),
                entry.display()
            ),
            ConfigError::Relative { path, entry } => write!(
                f,
                "{}: {} is not an absolute path",
                path.display(),
                entry.display()
            ),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigError::Read { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
            ConfigError::NoHome { .. } | ConfigError::Relative { .. } => None,
        }
    }
}

impl Config {
    /// An absent file is an empty config: `sync <path>` and the hook work
    /// without one.
    pub fn load(file: &Path, home: Option<&Path>) -> Result<Config, ConfigError> {
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: file.to_path_buf(),
                    source,
                });
            }
        };
        Config::parse(&text, file, home)
    }

    fn parse(text: &str, file: &Path, home: Option<&Path>) -> Result<Config, ConfigError> {
        let raw: Raw = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: file.to_path_buf(),
            source,
        })?;
        // launchd and systemd start callisto in an arbitrary directory, so
        // a relative entry has no fixed meaning.
        let resolve = |entry: PathBuf| match expand_tilde(&entry, home) {
            None => Err(ConfigError::NoHome {
                path: file.to_path_buf(),
                entry,
            }),
            Some(p) if p.is_relative() => Err(ConfigError::Relative {
                path: file.to_path_buf(),
                entry,
            }),
            Some(p) => Ok(p),
        };
        Ok(Config {
            vault: raw.vault.map(&resolve).transpose()?,
            repos: raw
                .repos
                .into_iter()
                .map(&resolve)
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum VaultError {
    NoHome,
    Unset,
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::NoHome => f.write_str("cannot find the vault: HOME is not set"),
            VaultError::Unset => f.write_str("no vault set"),
        }
    }
}

impl Error for VaultError {}

/// The `--vault` flag, then the config file. There is no default. A
/// relative flag stays relative: the user typed it in a shell.
pub fn vault(
    flag: Option<&Path>,
    config: &Config,
    home: Option<&Path>,
) -> Result<PathBuf, VaultError> {
    match (flag, &config.vault) {
        (Some(flag), _) => expand_tilde(flag, home),
        (None, Some(vault)) => Some(vault.clone()),
        (None, None) => return Err(VaultError::Unset),
    }
    .ok_or(VaultError::NoHome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn home() -> Option<&'static Path> {
        Some(Path::new("/h"))
    }

    fn load_text(text: &str, home: Option<&Path>) -> (PathBuf, Result<Config, ConfigError>) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        fs::write(&file, text).unwrap();
        let result = Config::load(&file, home);
        (file, result)
    }

    #[test]
    fn absent_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("config.toml"), home()).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn reads_vault_and_repos() {
        let text = r#"
vault = "~/org"
repos = [
  "~/Workspaces/ganymede",
  "~/Workspaces/b",
]
"#;
        let (_, config) = load_text(text, home());
        assert_eq!(
            config.unwrap(),
            Config {
                vault: Some("/h/org".into()),
                repos: vec!["/h/Workspaces/ganymede".into(), "/h/Workspaces/b".into()],
            }
        );
    }

    #[test]
    fn empty_file_is_empty() {
        let (_, config) = load_text("", home());
        assert_eq!(config.unwrap(), Config::default());
    }

    #[test]
    fn malformed_file_is_an_error() {
        let (file, config) = load_text("repos = [", home());
        let err = config.unwrap_err();
        assert!(matches!(&err, ConfigError::Parse { path, .. } if *path == file));
        assert!(err.to_string().starts_with(&file.display().to_string()));
    }

    #[test]
    fn unknown_key_is_an_error() {
        let (_, config) = load_text(r#"repo = ["/x"]"#, home());
        assert!(matches!(config, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn wrong_type_is_an_error() {
        let (_, config) = load_text(r#"repos = "/x""#, home());
        assert!(matches!(config, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn unreadable_config_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        fs::create_dir(&file).unwrap();
        let config = Config::load(&file, home());
        assert!(matches!(config, Err(ConfigError::Read { .. })));
    }

    #[test]
    fn tilde_without_home_is_an_error() {
        let (_, config) = load_text(r#"vault = "~/org""#, None);
        assert!(
            matches!(config, Err(ConfigError::NoHome { entry, .. }) if entry == Path::new("~/org"))
        );
    }

    #[test]
    fn relative_entry_is_an_error() {
        let (_, config) = load_text(r#"repos = ["Workspaces/ganymede"]"#, home());
        assert!(
            matches!(config, Err(ConfigError::Relative { entry, .. }) if entry == Path::new("Workspaces/ganymede"))
        );
    }

    #[test]
    fn absolute_entries_need_no_home() {
        let (_, config) = load_text("vault = \"/v\"\nrepos = [\"/r\"]", None);
        assert_eq!(
            config.unwrap(),
            Config {
                vault: Some("/v".into()),
                repos: vec!["/r".into()],
            }
        );
    }

    fn cfg() -> Config {
        Config {
            vault: Some("/cfg".into()),
            repos: vec![],
        }
    }

    #[test]
    fn flag_beats_config() {
        assert_eq!(
            vault(Some(Path::new("/flag")), &cfg(), home()),
            Ok("/flag".into())
        );
    }

    #[test]
    fn flag_expands_tilde() {
        assert_eq!(
            vault(Some(Path::new("~/v")), &cfg(), home()),
            Ok("/h/v".into())
        );
    }

    #[test]
    fn config_vault_is_used() {
        assert_eq!(vault(None, &cfg(), home()), Ok("/cfg".into()));
    }

    #[test]
    fn no_vault_is_unset() {
        assert_eq!(
            vault(None, &Config::default(), home()),
            Err(VaultError::Unset)
        );
        assert_eq!(
            vault(None, &Config::default(), None),
            Err(VaultError::Unset)
        );
    }

    #[test]
    fn flag_tilde_needs_home() {
        assert_eq!(
            vault(Some(Path::new("~/v")), &Config::default(), None),
            Err(VaultError::NoHome)
        );
    }

    #[test]
    fn config_vault_needs_no_home() {
        assert_eq!(vault(None, &cfg(), None), Ok("/cfg".into()));
    }
}
