//! Configured repos grouped by the beads database they use.
//!
//! Repos that redirect to one database share its issues and its prefix,
//! so they sync as one.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::bd::{Bd, BdError};
use crate::repo::{self, PREFIX_KEY, Repo};
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

/// The canonical database directory `repo` uses, following any redirect.
pub fn database(bd: &Bd, repo: &Repo) -> Result<PathBuf, ContextError> {
    let context = bd.context(&repo.root).map_err(ContextError::Bd)?;
    fs::canonicalize(&context.beads_dir).map_err(|source| ContextError::Canonicalize {
        path: context.beads_dir,
        source,
    })
}

#[derive(Debug)]
pub struct Group {
    pub database: PathBuf,
    /// In config order, each root once.
    pub members: Vec<Repo>,
    pub prefix: Option<String>,
}

impl Group {
    /// The checkout to run bd in for this group.
    pub fn representative<'a>(&'a self, target: Option<&'a Repo>) -> &'a Repo {
        match target {
            Some(t) if self.members.iter().any(|m| m.root == t.root) => t,
            _ => &self.members[0],
        }
    }
}

/// A configured path that belongs to no group.
#[derive(Debug)]
pub struct Ungrouped {
    pub path: PathBuf,
    /// `None` when the path is in no checkout.
    pub repo: Option<Repo>,
    /// `None` when the path is in no checkout.
    pub error: Option<ContextError>,
}

#[derive(Debug, Default)]
pub struct Groups {
    pub groups: Vec<Group>,
    pub ungrouped: Vec<Ungrouped>,
    /// Each configured path, with the index of its group.
    configured: Vec<(PathBuf, Option<usize>)>,
}

impl Groups {
    pub fn derive(bd: &Bd, paths: &[PathBuf]) -> Groups {
        let mut g = Groups::default();
        for path in paths {
            let index = g.place(bd, path);
            g.configured.push((path.clone(), index));
        }
        g
    }

    fn place(&mut self, bd: &Bd, path: &Path) -> Option<usize> {
        let Some(repo) = repo::locate(path) else {
            self.ungrouped.push(Ungrouped {
                path: path.to_path_buf(),
                repo: None,
                error: None,
            });
            return None;
        };
        if let Some(i) = self.index_of_member(&repo.root) {
            return Some(i);
        }
        let database = match database(bd, &repo) {
            Ok(d) => d,
            Err(e) => {
                self.ungrouped.push(Ungrouped {
                    path: path.to_path_buf(),
                    repo: Some(repo),
                    error: Some(e),
                });
                return None;
            }
        };
        match self.groups.iter().position(|g| g.database == database) {
            Some(i) => {
                self.groups[i].members.push(repo);
                Some(i)
            }
            None => {
                self.groups.push(Group {
                    database,
                    members: vec![repo],
                    prefix: None,
                });
                Some(self.groups.len() - 1)
            }
        }
    }

    pub fn read_prefixes(&mut self, bd: &Bd) {
        for group in &mut self.groups {
            group.prefix = bd
                .config_get(&group.members[0].root, PREFIX_KEY)
                .ok()
                .flatten();
        }
    }

    pub fn scope(&self) -> Scope {
        let prefixes = self
            .groups
            .iter()
            .filter_map(|g| g.prefix.clone())
            .collect();
        let unreadable = self
            .configured
            .iter()
            .filter(|(_, index)| index.is_none_or(|i| self.groups[i].prefix.is_none()))
            .map(|(path, _)| path.clone())
            .collect();
        Scope {
            prefixes,
            unreadable,
        }
    }

    fn index_of_member(&self, root: &Path) -> Option<usize> {
        self.groups
            .iter()
            .position(|g| g.members.iter().any(|m| m.root == root))
    }

    pub fn of_member(&self, root: &Path) -> Option<&Group> {
        self.index_of_member(root).map(|i| &self.groups[i])
    }

    /// The group whose database `target` uses, member or not.
    pub fn of_target(&self, bd: &Bd, target: &Repo) -> Result<Option<&Group>, ContextError> {
        if let Some(g) = self.of_member(&target.root) {
            return Ok(Some(g));
        }
        let database = database(bd, target)?;
        Ok(self.groups.iter().find(|g| g.database == database))
    }
}
