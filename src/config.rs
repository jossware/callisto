//! The config file: which repos callisto syncs and where the vault is.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::de::value::MapAccessDeserializer;
use serde::{Deserialize, Deserializer, de};

use crate::paths::expand_tilde;

const DEFAULT_UNFILED: &str = "Inbox";

/// Paths are expanded and absolute.
#[derive(Debug, PartialEq)]
pub struct Config {
    pub vault: Option<PathBuf>,
    pub repos: Vec<RepoEntry>,
    pub field: Option<String>,
    pub unfiled: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vault: None,
            repos: Vec::new(),
            field: None,
            unfiled: PathBuf::from(DEFAULT_UNFILED),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RepoEntry {
    Path(PathBuf),
    Table(TableEntry),
}

impl RepoEntry {
    pub fn path(&self) -> &Path {
        match self {
            RepoEntry::Path(path) => path,
            RepoEntry::Table(table) => &table.path,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableEntry {
    pub path: PathBuf,
    pub field: String,
    pub value: Option<Vec<String>>,
    pub project: Option<String>,
}

// A typo like `repo =` must fail loudly rather than empty the repo list.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    vault: Option<PathBuf>,
    #[serde(default)]
    repos: Vec<RawEntry>,
    field: Option<String>,
    unfiled: Option<PathBuf>,
}

enum RawEntry {
    Path(PathBuf),
    Table(RawTable),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTable {
    path: PathBuf,
    field: Option<String>,
    value: Option<OneOrMany>,
    project: Option<String>,
}

struct OneOrMany(Vec<String>);

// Hand-written rather than untagged: an untagged enum reports "did not
// match any variant" and hides which key was wrong.
impl<'de> Deserialize<'de> for RawEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = RawEntry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a path or a table")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<RawEntry, E> {
                Ok(RawEntry::Path(PathBuf::from(v)))
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<RawEntry, A::Error> {
                RawTable::deserialize(MapAccessDeserializer::new(map)).map(RawEntry::Table)
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl<'de> Deserialize<'de> for OneOrMany {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = OneOrMany;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string or a list of strings")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<OneOrMany, E> {
                Ok(OneOrMany(vec![v.to_owned()]))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<OneOrMany, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<String>()? {
                    items.push(item);
                }
                Ok(OneOrMany(items))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
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
    NoField {
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
            ConfigError::NoField { path, entry } => write!(
                f,
                "{}: {}: no field set: set field on the entry or at the top level",
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
            ConfigError::NoHome { .. }
            | ConfigError::Relative { .. }
            | ConfigError::NoField { .. }
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
        if let Some(field) = &raw.field {
            check_field(field).map_err(|r| invalid("field", r))?;
        }
        let unfiled = raw
            .unfiled
            .unwrap_or_else(|| PathBuf::from(DEFAULT_UNFILED));
        check_unfiled(&unfiled).map_err(|r| invalid("unfiled", r))?;
        let mut repos = Vec::with_capacity(raw.repos.len());
        for (i, entry) in raw.repos.into_iter().enumerate() {
            repos.push(match entry {
                RawEntry::Path(path) => RepoEntry::Path(resolve(path)?),
                RawEntry::Table(table) => {
                    let path = resolve(table.path)?;
                    let field = table.field.or_else(|| raw.field.clone()).ok_or_else(|| {
                        ConfigError::NoField {
                            path: file.to_path_buf(),
                            entry: path.clone(),
                        }
                    })?;
                    let key = |name: &str| format!("repos[{i}].{name}");
                    check_field(&field).map_err(|r| invalid(&key("field"), r))?;
                    if let Some(value) = &table.value
                        && (value.0.is_empty() || value.0.iter().any(String::is_empty))
                    {
                        return Err(invalid(&key("value"), "must be non-empty strings"));
                    }
                    if table.project.as_deref() == Some("") {
                        return Err(invalid(&key("project"), "must not be empty"));
                    }
                    RepoEntry::Table(TableEntry {
                        path,
                        field,
                        value: table.value.map(|v| v.0),
                        project: table.project,
                    })
                }
            });
        }
        Ok(Config {
            vault: raw.vault.map(&resolve).transpose()?,
            repos,
            field: raw.field,
            unfiled,
        })
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.repos.iter().map(|r| r.path().to_path_buf()).collect()
    }
}

fn check_field(field: &str) -> Result<(), &'static str> {
    if field.split('.').any(str::is_empty) {
        return Err("must be a dotted path with no empty segments");
    }
    Ok(())
}

// Relative to the vault root, so it must not escape or depend on the cwd.
fn check_unfiled(unfiled: &Path) -> Result<(), &'static str> {
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
                repos: vec![
                    RepoEntry::Path("/h/Workspaces/ganymede".into()),
                    RepoEntry::Path("/h/Workspaces/b".into()),
                ],
                field: None,
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
    fn reads_mixed_entries() {
        let text = r#"
field = "metadata.repo"
repos = [
  "~/a",
  { path = "~/b", value = "github.com/o/b", project = "B" },
  { path = "/c", field = "x.y", value = ["o/c", "o/c2"] },
]
"#;
        let (_, config) = load_text(text, home());
        assert_eq!(
            config.unwrap().repos,
            vec![
                RepoEntry::Path("/h/a".into()),
                RepoEntry::Table(TableEntry {
                    path: "/h/b".into(),
                    field: "metadata.repo".into(),
                    value: Some(vec!["github.com/o/b".into()]),
                    project: Some("B".into()),
                }),
                RepoEntry::Table(TableEntry {
                    path: "/c".into(),
                    field: "x.y".into(),
                    value: Some(vec!["o/c".into(), "o/c2".into()]),
                    project: None,
                }),
            ]
        );
    }

    #[test]
    fn table_value_and_project_optional() {
        let text = "field = \"f\"\nrepos = [{ path = \"/b\" }]";
        let (_, config) = load_text(text, home());
        assert_eq!(
            config.unwrap().repos,
            vec![RepoEntry::Table(TableEntry {
                path: "/b".into(),
                field: "f".into(),
                value: None,
                project: None,
            })]
        );
    }

    #[test]
    fn table_without_field_is_an_error() {
        let (_, config) = load_text(r#"repos = [{ path = "/b" }]"#, home());
        assert!(
            matches!(&config, Err(ConfigError::NoField { entry, .. }) if entry == Path::new("/b"))
        );
        assert!(config.unwrap_err().to_string().contains("no field set"));
    }

    #[test]
    fn unknown_key_in_table_names_it() {
        let text = "field = \"f\"\nrepos = [{ path = \"/b\", projet = \"B\" }]";
        let (_, config) = load_text(text, home());
        let err = config.unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        let text = err.to_string();
        assert!(text.contains("projet"), "{text}");
        assert!(!text.contains("did not match any variant"), "{text}");
    }

    #[test]
    fn unknown_top_level_key_names_it() {
        let (_, config) = load_text(r#"repo = ["/x"]"#, home());
        assert!(config.unwrap_err().to_string().contains("repo"));
    }

    #[test]
    fn wrong_entry_type_is_an_error() {
        for text in ["repos = [1]", r#"repos = [["/x"]]"#] {
            let (_, config) = load_text(text, home());
            let err = config.unwrap_err();
            assert!(matches!(err, ConfigError::Parse { .. }));
            assert!(err.to_string().contains("path or a table"), "{err}");
        }
    }

    #[test]
    fn wrong_value_type_is_an_error() {
        let text = "field = \"f\"\nrepos = [{ path = \"/b\", value = 1 }]";
        let (_, config) = load_text(text, home());
        assert!(matches!(config, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn table_path_is_expanded_and_absolute() {
        let text = "field = \"f\"\nrepos = [{ path = \"rel\" }]";
        let (_, config) = load_text(text, home());
        assert!(matches!(config, Err(ConfigError::Relative { .. })));
        let text = "field = \"f\"\nrepos = [{ path = \"~/b\" }]";
        let (_, config) = load_text(text, None);
        assert!(matches!(config, Err(ConfigError::NoHome { .. })));
    }

    #[test]
    fn missing_checkout_loads() {
        let text = "field = \"f\"\nrepos = [{ path = \"/does/not/exist\", value = \"v\" }]";
        let (_, config) = load_text(text, home());
        assert!(config.is_ok());
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
    fn bad_field_is_an_error() {
        for bad in ["", "a..b", ".a", "a."] {
            assert_eq!(invalid_key(&format!("field = \"{bad}\"")), "field");
            let text = format!("repos = [{{ path = \"/b\", field = \"{bad}\" }}]");
            assert_eq!(invalid_key(&text), "repos[0].field");
        }
    }

    #[test]
    fn empty_value_is_an_error() {
        for bad in [r#""""#, "[]", r#"["a", ""]"#] {
            let text = format!("field = \"f\"\nrepos = [{{ path = \"/b\", value = {bad} }}]");
            assert_eq!(invalid_key(&text), "repos[0].value");
        }
    }

    #[test]
    fn empty_project_is_an_error() {
        let text = "field = \"f\"\nrepos = [{ path = \"/b\", project = \"\" }]";
        assert_eq!(invalid_key(text), "repos[0].project");
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
        let (file, config) = load_text(r#"field = "a..b""#, home());
        let text = config.unwrap_err().to_string();
        assert!(
            text.starts_with(&format!("{}: field: ", file.display())),
            "{text}"
        );
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
                repos: vec![RepoEntry::Path("/r".into())],
                field: None,
                unfiled: "Inbox".into(),
            }
        );
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
