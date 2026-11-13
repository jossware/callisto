//! The beads databases callisto syncs, as configured.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::bd::{Bd, BdError};
use crate::config::{DatabaseEntry, Filing};
use crate::repo::{PREFIX_KEY, Repo};
use crate::standing::Condition;
use crate::sync::Scope;

#[derive(Debug)]
pub enum ContextError {
    Bd(BdError),
    Canonicalize { path: PathBuf, source: io::Error },
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContextError::Bd(e) => e.fmt(f),
            ContextError::Canonicalize { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
        }
    }
}

impl Error for ContextError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ContextError::Bd(e) => Some(e),
            ContextError::Canonicalize { source, .. } => Some(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Database {
    /// Canonical when it exists, else as configured.
    pub dir: PathBuf,
    /// File name of `dir`'s parent: the default project name.
    pub name: String,
    pub filing: Filing,
}

/// The configured databases, in config order.
pub struct Databases {
    databases: Vec<Database>,
    /// Parallel to `databases`; `None` until `read_prefixes`.
    prefixes: Vec<Option<Result<String, String>>>,
}

impl Databases {
    pub fn new(entries: &[DatabaseEntry]) -> Databases {
        let databases: Vec<Database> = entries
            .iter()
            .map(|entry| {
                let dir =
                    fs::canonicalize(&entry.beads_dir).unwrap_or_else(|_| entry.beads_dir.clone());
                Database {
                    name: name_of(&dir),
                    dir,
                    filing: entry.filing.clone(),
                }
            })
            .collect();
        let prefixes = vec![None; databases.len()];
        Databases {
            databases,
            prefixes,
        }
    }

    pub fn all(&self) -> &[Database] {
        &self.databases
    }

    pub fn read_prefixes(&mut self, bd: &Bd) {
        for (database, prefix) in self.databases.iter().zip(&mut self.prefixes) {
            *prefix = Some(if database.dir.is_dir() {
                match bd.config_get_on(&database.dir, PREFIX_KEY) {
                    Ok(Some(p)) => Ok(p),
                    Ok(None) => Err(format!("bd config has no {PREFIX_KEY}")),
                    Err(e) => Err(e.to_string()),
                }
            } else {
                Err("no such directory".to_string())
            });
        }
    }

    pub fn prefix(&self, dir: &Path) -> Option<&str> {
        known(&self.prefixes[self.index(dir)?])
    }

    pub fn scope(&self) -> Scope {
        let mut prefixes: Vec<String> = Vec::new();
        let mut shared: Vec<String> = Vec::new();
        for p in self.prefixes.iter().filter_map(known) {
            if !prefixes.iter().any(|q| q == p) {
                prefixes.push(p.to_string());
            } else if !shared.iter().any(|s| s == p) {
                shared.push(p.to_string());
            }
        }
        let unreadable = self
            .databases
            .iter()
            .zip(&self.prefixes)
            .filter(|(_, p)| known(p).is_none())
            .map(|(d, _)| d.dir.clone())
            .collect();
        Scope {
            prefixes,
            unreadable,
            shared,
        }
    }

    pub fn conditions(&self) -> Vec<Condition> {
        let mut conditions: Vec<Condition> = self
            .scope()
            .shared
            .into_iter()
            .map(|prefix| Condition::SharedPrefix {
                databases: self
                    .databases
                    .iter()
                    .zip(&self.prefixes)
                    .filter(|(_, p)| known(p) == Some(prefix.as_str()))
                    .map(|(d, _)| d.dir.clone())
                    .collect(),
                prefix,
            })
            .collect();
        for (database, prefix) in self.databases.iter().zip(&self.prefixes) {
            if let Some(Err(reason)) = prefix {
                conditions.push(Condition::Unreadable {
                    database: database.dir.clone(),
                    reason: reason.clone(),
                });
            }
        }
        conditions
    }

    pub fn configured(&self, dir: &Path) -> Option<&Database> {
        self.index(dir).map(|i| &self.databases[i])
    }

    fn index(&self, dir: &Path) -> Option<usize> {
        self.databases.iter().position(|d| d.dir == dir)
    }
}

fn known(prefix: &Option<Result<String, String>>) -> Option<&str> {
    match prefix {
        Some(Ok(p)) => Some(p),
        _ => None,
    }
}

fn name_of(dir: &Path) -> String {
    dir.parent().and_then(Path::file_name).map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Which database bd would use for a checkout, and how callisto knows it.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Configured(Database),
    /// The checkout's own database, unconfigured.
    Local(Database),
    Unconfigured(PathBuf),
}

pub fn find(bd: &Bd, databases: &Databases, checkout: &Repo) -> Result<Target, ContextError> {
    let context = bd.lookup(&checkout.root).map_err(ContextError::Bd)?;
    let dir =
        fs::canonicalize(&context.beads_dir).map_err(|source| ContextError::Canonicalize {
            path: context.beads_dir,
            source,
        })?;
    if let Some(database) = databases.configured(&dir) {
        return Ok(Target::Configured(database.clone()));
    }
    if fs::canonicalize(checkout.root.join(".beads")).is_ok_and(|own| own == dir) {
        return Ok(Target::Local(Database {
            name: name_of(&dir),
            dir,
            filing: Filing::Single { project: None },
        }));
    }
    Ok(Target::Unconfigured(dir))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::bd::test_support::fake_bd;

    /// Answers `context` and `config get` for `$BEADS_DIR`, else for the
    /// working directory's `.beads`, whose `redirect` file `context`
    /// follows. Logs every call to `calls` beside itself.
    fn bd(tools: &Path) -> Bd {
        let calls = tools.join("calls");
        Bd::new(fake_bd(
            tools,
            &format!(
                r#"echo "$*" >> '{}'
d="${{BEADS_DIR:-$(pwd -P)/.beads}}"
case "$1" in
context)
  if [ -z "$BEADS_DIR" ] && [ -f "$d/redirect" ]; then d="$(cat "$d/redirect")"; fi
  printf '{{"beads_dir":"%s"}}\n' "$d" ;;
config)
  printf '{{"key":"%s","value":"%s"}}\n' "$4" "$(cat "$d/prefix" 2>/dev/null)" ;;
*) exit 9 ;;
esac"#,
                calls.display()
            ),
        ))
    }

    fn entry(dir: impl Into<PathBuf>) -> DatabaseEntry {
        DatabaseEntry {
            beads_dir: dir.into(),
            filing: Filing::Single { project: None },
        }
    }

    /// A checkout at `root/name` with its own `.beads`.
    fn checkout(root: &Path, name: &str) -> Repo {
        let path = root.join(name);
        fs::create_dir_all(path.join(".beads")).unwrap();
        Repo {
            root: path.canonicalize().unwrap(),
            name: name.to_string(),
        }
    }

    fn database(root: &Path, name: &str, prefix: &str) -> PathBuf {
        let dir = root.join(name).join(".beads");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("prefix"), prefix).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn name_is_the_parent_directory() {
        let dbs = Databases::new(&[entry("/h/x/.beads"), entry("/h/.dotfiles/.beads")]);
        let names: Vec<&str> = dbs.all().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["x", ".dotfiles"]);
        assert_eq!(dbs.all()[0].dir, PathBuf::from("/h/x/.beads"));
    }

    #[test]
    fn symlinked_beads_dir_matches_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let repo = checkout(tmp.path(), "real");
        symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        let dbs = Databases::new(&[entry(tmp.path().join("link/.beads"))]);
        let found = find(&bd(tools.path()), &dbs, &repo).unwrap();
        let dir = repo.root.join(".beads");
        assert_eq!(
            found,
            Target::Configured(Database {
                dir: dir.clone(),
                name: "real".into(),
                filing: Filing::Single { project: None },
            })
        );
        assert!(dbs.configured(&dir).is_some());
    }

    #[test]
    fn trailing_slash_beads_dir_matches_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let repo = checkout(tmp.path(), "x");
        let configured = format!("{}/", tmp.path().join("x/.beads").display());
        let dbs = Databases::new(&[DatabaseEntry {
            beads_dir: configured.into(),
            filing: Filing::Single {
                project: Some("P".into()),
            },
        }]);
        let Target::Configured(db) = find(&bd(tools.path()), &dbs, &repo).unwrap() else {
            panic!("not configured");
        };
        assert_eq!(db.dir, repo.root.join(".beads"));
        assert_eq!(db.name, "x");
    }

    #[test]
    fn checkout_own_unconfigured_db_is_local() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let repo = checkout(tmp.path(), "x");
        let other = database(tmp.path(), "y", "y");
        let dbs = Databases::new(&[entry(other)]);
        assert_eq!(
            find(&bd(tools.path()), &dbs, &repo).unwrap(),
            Target::Local(Database {
                dir: repo.root.join(".beads"),
                name: "x".into(),
                filing: Filing::Single { project: None },
            })
        );
    }

    #[test]
    fn other_unconfigured_db_is_unconfigured() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let repo = checkout(tmp.path(), "x");
        let elsewhere = database(tmp.path(), "shared", "s");
        fs::write(
            repo.root.join(".beads/redirect"),
            tmp.path()
                .join("shared/.beads")
                .as_os_str()
                .as_encoded_bytes(),
        )
        .unwrap();
        let dbs = Databases::new(&[]);
        assert_eq!(
            find(&bd(tools.path()), &dbs, &repo).unwrap(),
            Target::Unconfigured(elsewhere)
        );
    }

    #[test]
    fn shared_prefix_is_a_condition_and_in_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let a = database(tmp.path(), "a", "p");
        let b = database(tmp.path(), "b", "q");
        let c = database(tmp.path(), "c", "p");
        let mut dbs = Databases::new(&[entry(&a), entry(&b), entry(&c)]);
        dbs.read_prefixes(&bd(tools.path()));
        assert_eq!(dbs.prefix(&a), Some("p"));
        assert_eq!(dbs.prefix(&b), Some("q"));
        let scope = dbs.scope();
        assert_eq!(scope.prefixes, ["p", "q"]);
        assert_eq!(scope.shared, ["p"]);
        assert!(scope.unreadable.is_empty(), "{:?}", scope.unreadable);
        assert_eq!(
            dbs.conditions(),
            [Condition::SharedPrefix {
                prefix: "p".into(),
                databases: vec![a, c],
            }]
        );
    }

    #[test]
    fn missing_dir_is_unreadable_and_runs_no_bd() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("gone/.beads");
        let mut dbs = Databases::new(&[entry(&missing)]);
        dbs.read_prefixes(&bd(tools.path()));
        assert!(!tools.path().join("calls").exists());
        assert_eq!(dbs.prefix(&missing), None);
        assert_eq!(dbs.scope().unreadable, std::slice::from_ref(&missing));
        assert_eq!(
            dbs.conditions(),
            [Condition::Unreadable {
                database: missing,
                reason: "no such directory".into(),
            }]
        );
    }

    #[test]
    fn prefixless_db_is_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let dir = database(tmp.path(), "a", "");
        let mut dbs = Databases::new(&[entry(&dir)]);
        dbs.read_prefixes(&bd(tools.path()));
        assert_eq!(dbs.scope().unreadable, std::slice::from_ref(&dir));
        assert_eq!(
            dbs.conditions(),
            [Condition::Unreadable {
                database: dir,
                reason: format!("bd config has no {PREFIX_KEY}"),
            }]
        );
    }
}
