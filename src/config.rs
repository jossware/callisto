//! The config file: which databases callisto syncs and where the vault
//! is.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::field_path::FieldPath;
use crate::paths::expand_tilde;

const DEFAULT_UNFILED: &str = "Inbox";

/// Vault and database paths are expanded and absolute. `unfiled` is
/// relative to the vault.
#[derive(Debug, PartialEq)]
pub struct Config {
    pub vault: Option<PathBuf>,
    pub databases: Vec<DatabaseEntry>,
    pub unfiled: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vault: None,
            databases: Vec::new(),
            unfiled: PathBuf::from(DEFAULT_UNFILED),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseEntry {
    pub beads_dir: PathBuf,
    pub filing: Filing,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Filing {
    Single { project: Option<String> },
    Split(Split),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub by: FieldPath,
    pub projects: BTreeMap<String, String>,
}

// A typo like `database =` must fail loudly rather than empty the list.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    vault: Option<PathBuf>,
    #[serde(default)]
    databases: Vec<RawDatabase>,
    unfiled: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDatabase {
    beads_dir: PathBuf,
    project: Option<String>,
    split: Option<RawSplit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSplit {
    by: String,
    #[serde(default)]
    projects: BTreeMap<String, String>,
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
    Invalid {
        path: PathBuf,
        key: String,
        reason: String,
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
            ConfigError::Invalid { path, key, reason } => {
                write!(f, "{}: {key}: {reason}", path.display())
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigError::Read { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
            ConfigError::NoHome { .. }
            | ConfigError::Relative { .. }
            | ConfigError::Invalid { .. } => None,
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
        let invalid = |key: &str, reason: &str| ConfigError::Invalid {
            path: file.to_path_buf(),
            key: key.to_owned(),
            reason: reason.to_owned(),
        };
        let unfiled = raw
            .unfiled
            .unwrap_or_else(|| PathBuf::from(DEFAULT_UNFILED));
        check_unfiled(&unfiled).map_err(|r| invalid("unfiled", r))?;
        let vault = raw.vault.map(resolve).transpose()?;
        let mut databases: Vec<DatabaseEntry> = Vec::with_capacity(raw.databases.len());
        for (i, entry) in raw.databases.into_iter().enumerate() {
            let key = |name: &str| format!("databases[{i}].{name}");
            let beads_dir = resolve(entry.beads_dir)?;
            let filing = match (entry.project, entry.split) {
                (Some(_), Some(_)) => {
                    return Err(invalid(
                        &format!("databases[{i}]"),
                        "must not set both project and split",
                    ));
                }
                (project, None) => {
                    if project.as_deref() == Some("") {
                        return Err(invalid(&key("project"), "must not be empty"));
                    }
                    Filing::Single { project }
                }
                (None, Some(split)) => {
                    let by = split
                        .by
                        .parse::<FieldPath>()
                        .map_err(|e| invalid(&key("split.by"), &e.to_string()))?;
                    if split
                        .projects
                        .iter()
                        .any(|(value, project)| value.is_empty() || project.is_empty())
                    {
                        return Err(invalid(
                            &key("split.projects"),
                            "must map non-empty values to non-empty project names",
                        ));
                    }
                    Filing::Split(Split {
                        by,
                        projects: split.projects,
                    })
                }
            };
            if let Some(first) = databases
                .iter()
                .position(|d| same_database(&d.beads_dir, &beads_dir))
            {
                return Err(invalid(
                    &key("beads_dir"),
                    &format!("is the same database as databases[{first}]"),
                ));
            }
            databases.push(DatabaseEntry { beads_dir, filing });
        }
        Ok(Config {
            vault,
            databases,
            unfiled,
        })
    }
}

fn same_database(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

// Relative to the vault root, so it must not escape or depend on the cwd.
fn check_unfiled(unfiled: &Path) -> Result<(), &'static str> {
    // Path::components normalises `a/./b` and a trailing `/`, so check the
    // raw segments.
    if let Some(raw) = unfiled.to_str()
        && raw.split('/').any(|s| s.is_empty() || s == ".")
    {
        return Err("must be a relative path inside the vault");
    }
    let mut components = unfiled.components().peekable();
    if components.peek().is_none() || !components.all(|c| matches!(c, Component::Normal(_))) {
        return Err("must be a relative path inside the vault");
    }
    Ok(())
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
    fn reads_vault_and_databases() {
        let text = r#"
vault = "~/org"

[[databases]]
beads_dir = "~/Workspaces/ganymede/.beads"
"#;
        let (_, config) = load_text(text, home());
        assert_eq!(
            config.unwrap(),
            Config {
                vault: Some("/h/org".into()),
                databases: vec![DatabaseEntry {
                    beads_dir: "/h/Workspaces/ganymede/.beads".into(),
                    filing: Filing::Single { project: None },
                }],
                unfiled: "Inbox".into(),
            }
        );
    }

    #[test]
    fn empty_file_is_empty() {
        let (_, config) = load_text("", home());
        assert_eq!(config.unwrap(), Config::default());
    }

    #[test]
    fn unknown_top_level_key_names_it() {
        let (_, config) = load_text(r#"database = ["/x"]"#, home());
        let err = config.unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
        assert!(err.to_string().contains("database"), "{err}");
    }

    #[test]
    fn unfiled_defaults_to_inbox() {
        assert_eq!(Config::default().unfiled, Path::new("Inbox"));
        let (_, config) = load_text("", home());
        assert_eq!(config.unwrap().unfiled, Path::new("Inbox"));
    }

    fn invalid_key(text: &str) -> String {
        match load_text(text, home()).1 {
            Err(ConfigError::Invalid { key, .. }) => key,
            other => panic!("expected Invalid for {text:?}, got {other:?}"),
        }
    }

    #[test]
    fn bad_unfiled_is_an_error() {
        for bad in ["", "/abs", "../x", "a/../b", "./a"] {
            assert_eq!(invalid_key(&format!("unfiled = \"{bad}\"")), "unfiled");
        }
    }

    #[test]
    fn nested_unfiled_is_kept() {
        for ok in ["Inbox/beads", "~/x"] {
            let (_, config) = load_text(&format!("unfiled = \"{ok}\""), home());
            assert_eq!(config.unwrap().unfiled, Path::new(ok));
        }
    }

    #[test]
    fn invalid_display_names_file_and_key() {
        let (file, config) = load_text(r#"unfiled = "/abs""#, home());
        let text = config.unwrap_err().to_string();
        assert!(
            text.starts_with(&format!("{}: unfiled: ", file.display())),
            "{text}"
        );
    }

    #[test]
    fn malformed_file_is_an_error() {
        let (file, config) = load_text("databases = [", home());
        let err = config.unwrap_err();
        assert!(matches!(&err, ConfigError::Parse { path, .. } if *path == file));
        assert!(err.to_string().starts_with(&file.display().to_string()));
    }

    #[test]
    fn old_repos_key_is_unknown() {
        let (_, config) = load_text("repos = []", home());
        let err = config.unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
        assert!(err.to_string().contains("repos"), "{err}");
    }

    #[test]
    fn old_field_key_is_unknown() {
        let (_, config) = load_text(r#"field = "metadata.repo""#, home());
        let err = config.unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
        assert!(err.to_string().contains("field"), "{err}");
    }

    #[test]
    fn wrong_type_is_an_error() {
        let (_, config) = load_text(r#"databases = "/x""#, home());
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
    fn vault_error_reported_before_databases() {
        let text = "vault = \"rel\"\n[[databases]]\nbeads_dir = \"also-rel\"";
        let (_, config) = load_text(text, home());
        assert!(
            matches!(config, Err(ConfigError::Relative { entry, .. }) if entry == Path::new("rel"))
        );
    }

    #[test]
    fn unfiled_with_empty_or_dot_segments_is_an_error() {
        for bad in ["a/./b", "Inbox/", "a//b"] {
            assert_eq!(invalid_key(&format!("unfiled = \"{bad}\"")), "unfiled");
        }
        let (_, config) = load_text(r#"unfiled = "Inbox/beads""#, home());
        assert_eq!(config.unwrap().unfiled, Path::new("Inbox/beads"));
    }

    #[test]
    fn absolute_entries_need_no_home() {
        let text = "vault = \"/v\"\n[[databases]]\nbeads_dir = \"/r/.beads\"";
        let (_, config) = load_text(text, None);
        assert_eq!(
            config.unwrap(),
            Config {
                vault: Some("/v".into()),
                databases: vec![DatabaseEntry {
                    beads_dir: "/r/.beads".into(),
                    filing: Filing::Single { project: None },
                }],
                unfiled: "Inbox".into(),
            }
        );
    }

    #[test]
    fn databases_parse_single_and_split() {
        let text = r#"
[[databases]]
beads_dir = "~/work/api/.beads"

[[databases]]
beads_dir = "/srv/b/.beads"
project = "B"

[[databases]]
beads_dir = "/srv/shared/.beads"
split.by = "metadata.repo"
split.projects = { "github.com/o/api" = "API" }
"#;
        let (_, config) = load_text(text, home());
        assert_eq!(
            config.unwrap().databases,
            vec![
                DatabaseEntry {
                    beads_dir: "/h/work/api/.beads".into(),
                    filing: Filing::Single { project: None },
                },
                DatabaseEntry {
                    beads_dir: "/srv/b/.beads".into(),
                    filing: Filing::Single {
                        project: Some("B".into())
                    },
                },
                DatabaseEntry {
                    beads_dir: "/srv/shared/.beads".into(),
                    filing: Filing::Split(Split {
                        by: "metadata.repo".parse().unwrap(),
                        projects: BTreeMap::from([("github.com/o/api".into(), "API".into())]),
                    }),
                },
            ]
        );
    }

    #[test]
    fn project_and_split_together_is_invalid() {
        let text = "[[databases]]\nbeads_dir = \"/a\"\nproject = \"P\"\nsplit.by = \"x\"";
        let (_, config) = load_text(text, home());
        let err = config.unwrap_err();
        assert!(
            matches!(&err, ConfigError::Invalid { key, .. } if key == "databases[0]"),
            "{err}"
        );
        let text = err.to_string();
        assert!(text.contains("project") && text.contains("split"), "{text}");
    }

    #[test]
    fn split_requires_by() {
        let text = "[[databases]]\nbeads_dir = \"/a\"\n[databases.split]\nprojects = {}";
        let (_, config) = load_text(text, home());
        let err = config.unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert!(err.to_string().contains("by"), "{err}");
    }

    #[test]
    fn unknown_key_in_split_is_named() {
        let text = "[[databases]]\nbeads_dir = \"/a\"\nsplit.bye = \"x\"";
        let (_, config) = load_text(text, home());
        let err = config.unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert!(err.to_string().contains("bye"), "{err}");
    }

    #[test]
    fn bad_database_values_are_invalid() {
        let head = "[[databases]]\nbeads_dir = \"/a\"\n";
        let cases = [
            ("project = \"\"", "databases[0].project"),
            ("split.by = \"a..b\"", "databases[0].split.by"),
            (
                "split.by = \"x\"\nsplit.projects = { \"\" = \"P\" }",
                "databases[0].split.projects",
            ),
            (
                "split.by = \"x\"\nsplit.projects = { v = \"\" }",
                "databases[0].split.projects",
            ),
        ];
        for (body, key) in cases {
            assert_eq!(invalid_key(&format!("{head}{body}")), key, "{body}");
        }
    }

    #[test]
    fn same_database_twice_is_invalid() {
        let text =
            "[[databases]]\nbeads_dir = \"~/x/.beads\"\n[[databases]]\nbeads_dir = \"/h/x/.beads\"";
        assert_eq!(invalid_key(text), "databases[1].beads_dir");
    }

    #[test]
    fn same_database_by_symlink_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let text = format!(
            "[[databases]]\nbeads_dir = \"{}\"\n[[databases]]\nbeads_dir = \"{}/\"",
            real.display(),
            link.display()
        );
        assert_eq!(invalid_key(&text), "databases[1].beads_dir");
    }

    #[test]
    fn relative_beads_dir_is_rejected() {
        let (_, config) = load_text("[[databases]]\nbeads_dir = \"rel/.beads\"", home());
        assert!(matches!(config, Err(ConfigError::Relative { .. })));
    }

    #[test]
    fn missing_beads_dir_still_loads() {
        let (_, config) = load_text("[[databases]]\nbeads_dir = \"/does/not/exist\"", home());
        assert!(config.is_ok());
    }

    fn cfg() -> Config {
        Config {
            vault: Some("/cfg".into()),
            ..Config::default()
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
