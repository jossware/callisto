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
use crate::config::{RepoEntry, TableEntry};
use crate::field_path::FieldPath;
use crate::remote::Git;
use crate::repo::{self, PREFIX_KEY, Repo};
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
    /// In config order; a root repeats when two entries name it.
    pub entries: Vec<Placed>,
    pub ownership: Ownership,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub repo: Repo,
    pub entry: RepoEntry,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub values: Vec<String>,
    pub project: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldClaims {
    pub field: FieldPath,
    pub claims: Vec<Claim>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ownership {
    Prefix,
    Field(FieldClaims),
    Broken(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Unresolvable {
    pub path: PathBuf,
    pub reason: String,
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
    /// In config order.
    pub unresolvable: Vec<Unresolvable>,
    /// Each configured entry, with the indexes of its group and of its
    /// place in that group's entries.
    configured: Vec<(RepoEntry, Option<(usize, usize)>)>,
}

impl Groups {
    pub fn derive(bd: &Bd, entries: &[RepoEntry]) -> Groups {
        let mut g = Groups::default();
        for entry in entries {
            let placed = g.place(bd, entry.path()).map(|(i, repo)| {
                let entries = &mut g.groups[i].entries;
                entries.push(Placed {
                    repo,
                    entry: entry.clone(),
                });
                (i, entries.len() - 1)
            });
            g.configured.push((entry.clone(), placed));
        }
        g
    }

    fn place(&mut self, bd: &Bd, path: &Path) -> Option<(usize, Repo)> {
        let Some(repo) = repo::locate(path) else {
            self.ungrouped.push(Ungrouped {
                path: path.to_path_buf(),
                repo: None,
                error: None,
            });
            return None;
        };
        if let Some(i) = self.index_of_member(&repo.root) {
            return Some((i, repo));
        }
        let failed = |u: &Ungrouped| u.repo.as_ref().is_some_and(|r| r.root == repo.root);
        if self.ungrouped.iter().any(failed) {
            return None;
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
                self.groups[i].members.push(repo.clone());
                Some((i, repo))
            }
            None => {
                self.groups.push(Group {
                    database,
                    members: vec![repo.clone()],
                    prefix: None,
                    entries: Vec::new(),
                    ownership: Ownership::Prefix,
                });
                Some((self.groups.len() - 1, repo))
            }
        }
    }

    /// Why `path`, which belongs to no group, could not be placed.
    fn unplaced_reason(&self, path: &Path) -> String {
        let Some(repo) = repo::locate(path) else {
            return "not in a repository".to_string();
        };
        self.ungrouped
            .iter()
            .find(|u| u.repo.as_ref().is_some_and(|r| r.root == repo.root))
            .and_then(|u| u.error.as_ref())
            .map_or_else(|| "not in a repository".to_string(), ToString::to_string)
    }

    pub fn read_prefixes(&mut self, bd: &Bd) {
        for group in &mut self.groups {
            group.prefix = bd
                .config_get(&group.members[0].root, PREFIX_KEY)
                .ok()
                .flatten();
        }
    }

    /// Defaults each table entry's value and project, and sets each
    /// group's ownership. Runs after `read_prefixes`.
    pub fn resolve(&mut self, git: &Git) {
        let mut unresolvable = Vec::new();
        let mut claims = vec![Vec::new(); self.groups.len()];
        for (entry, placed) in &self.configured {
            let RepoEntry::Table(table) = entry else {
                continue;
            };
            let result = match *placed {
                None => Err(self.unplaced_reason(&table.path)),
                Some((g, e)) => {
                    let group = &self.groups[g];
                    claim(group, &group.entries[e].repo, table, git).map(|c| claims[g].push(c))
                }
            };
            if let Err(reason) = result {
                unresolvable.push(Unresolvable {
                    path: table.path.clone(),
                    reason,
                });
            }
        }
        for (group, claims) in self.groups.iter_mut().zip(claims) {
            group.ownership = ownership(&group.entries, claims);
        }
        self.unresolvable = unresolvable;
    }

    pub fn degraded(&self) -> bool {
        !self.unresolvable.is_empty()
    }

    pub fn conditions(&self) -> Vec<Condition> {
        self.unresolvable
            .iter()
            .map(|u| Condition::Unresolvable {
                path: u.path.clone(),
                reason: u.reason.clone(),
            })
            .collect()
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
            .filter(|(entry, placed)| {
                let prefixless = |(g, _): (usize, usize)| self.groups[g].prefix.is_none();
                match entry {
                    RepoEntry::Path(_) => placed.is_none_or(prefixless),
                    // Reported as unresolvable rather than as an incomplete
                    // scope.
                    RepoEntry::Table(_) => placed.is_some_and(prefixless),
                }
            })
            .map(|(entry, _)| entry.path().to_path_buf())
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

fn claim(group: &Group, repo: &Repo, table: &TableEntry, git: &Git) -> Result<Claim, String> {
    if group.prefix.is_none() {
        return Err("cannot read the issue prefix".to_string());
    }
    let values = match &table.value {
        Some(values) => values.clone(),
        None => vec![git.origin(&repo.root).map_err(|e| e.to_string())?],
    };
    let project = table.project.clone().unwrap_or_else(|| repo.name.clone());
    Ok(Claim { values, project })
}

fn ownership(entries: &[Placed], claims: Vec<Claim>) -> Ownership {
    let tables: Vec<(&Repo, &TableEntry)> = entries
        .iter()
        .filter_map(|p| match &p.entry {
            RepoEntry::Table(t) => Some((&p.repo, t)),
            RepoEntry::Path(_) => None,
        })
        .collect();
    let Some((_, first)) = tables.first() else {
        return Ownership::Prefix;
    };
    if tables.len() < entries.len() {
        return Ownership::Broken("path and table entries share one database".to_string());
    }
    if let Some((_, other)) = tables.iter().find(|(_, t)| t.field != first.field) {
        return Ownership::Broken(format!(
            "table entries use different fields ({}, {})",
            first.field, other.field
        ));
    }
    for (i, (repo, _)) in tables.iter().enumerate() {
        if tables[..i].iter().any(|(r, _)| r.root == repo.root) {
            return Ownership::Broken(format!("{} is listed twice", repo.root.display()));
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for claim in &claims {
        if let Some(v) = claim.values.iter().find(|v| seen.contains(&v.as_str())) {
            return Ownership::Broken(format!("value \"{v}\" is claimed by more than one entry"));
        }
        seen.extend(claim.values.iter().map(String::as_str));
    }
    match first.field.parse() {
        Ok(field) => Ownership::Field(FieldClaims { field, claims }),
        Err(e) => Ownership::Broken(e.to_string()),
    }
}
