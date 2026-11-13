//! Syncing one repo's beads into the vault.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;

use crate::bd::Bd;
use crate::claim::{self, Unclaimed};
use crate::errlog::Stage;
use crate::export::{self, ExportError};
use crate::field_path::{FieldPath, FieldValue};
use crate::filename;
use crate::graph::Graph;
use crate::group::FieldClaims;
use crate::lock::Lock;
use crate::project::{self, Project, Unmapped};
use crate::removed;
use crate::render::{self, frontmatter::Context, link::Links, unfiled};
use crate::repo::{self, Repo, SettingsError};
use crate::standing::Condition;
use crate::vault::{self, Generated, Index, ScanError, owner};
use crate::write;

/// The issue prefixes of the configured repos, which decide which repo
/// a bead id belongs to.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub prefixes: Vec<String>,
    /// Configured repos whose prefix could not be read.
    pub unreadable: Vec<PathBuf>,
    /// Prefixes held by two or more configured databases.
    pub shared: Vec<String>,
}

pub struct Request<'a> {
    pub bd: &'a Bd,
    pub vault: &'a Path,
    pub repo: &'a Repo,
    pub scope: &'a Scope,
    pub tz: &'a TimeZone,
    /// `None` files every bead under the repo's own project.
    pub claims: Option<&'a FieldClaims>,
    /// Some table entry could not be resolved, so a bead missing from
    /// this export may belong to that entry.
    pub degraded: bool,
    /// Vault-relative folder for a table group's index of unclaimed beads.
    pub unfiled: &'a Path,
}

/// Paths are relative to the vault, in the order they were written.
#[derive(Debug, Default)]
pub struct Report {
    pub created: Vec<PathBuf>,
    pub updated: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub unchanged: usize,
    pub warnings: Vec<Warning>,
    pub conditions: Vec<Condition>,
    /// Beads no table entry files, in export order.
    pub unclaimed: Vec<Unclaimed>,
    pub degraded: bool,
}

#[derive(Debug)]
pub enum Warning {
    Unmapped(Unmapped),
    /// The repo is not in the configured list, so a longer prefix may own
    /// some of the ids it claims.
    UncheckedPrefix {
        prefix: String,
    },
    Duplicate {
        id: String,
        paths: Vec<PathBuf>,
    },
    /// A new note's filename belongs to a file callisto did not write.
    Taken {
        id: String,
        path: PathBuf,
    },
    /// A removed bead's note is not UTF-8, so it cannot be marked.
    NotUtf8 {
        id: String,
        path: PathBuf,
    },
    /// Removal was skipped: without every configured prefix, this repo
    /// could claim another repo's notes.
    ScopeIncomplete {
        repos: Vec<PathBuf>,
    },
    Degraded,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |paths: &[PathBuf]| {
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self {
            Warning::Unmapped(u) => u.fmt(f),
            Warning::UncheckedPrefix { prefix } => write!(
                f,
                "prefix \"{prefix}\" is not in the configured repo list, so it is unchecked"
            ),
            Warning::Duplicate { id, paths } => {
                write!(
                    f,
                    "{id}: {} generated notes ({}); none written",
                    paths.len(),
                    list(paths)
                )?;
                let conflict = paths.iter().any(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().contains(".sync-conflict-"))
                });
                if conflict {
                    f.write_str("; Syncthing conflict copies are a likely cause")?;
                }
                Ok(())
            }
            Warning::Taken { id, path } => write!(
                f,
                "{id}: {} already exists and callisto did not write it; skipped",
                path.display()
            ),
            Warning::NotUtf8 { id, path } => write!(
                f,
                "{id}: {} is not valid UTF-8, so it was not marked removed; fix its encoding to let callisto mark it",
                path.display()
            ),
            Warning::ScopeIncomplete { repos } => write!(
                f,
                "cannot read the prefix of {}; removed beads were not marked",
                list(repos)
            ),
            Warning::Degraded => {
                f.write_str("a table entry is unresolvable; removed beads were not marked")
            }
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Settings(SettingsError),
    NoVault(PathBuf),
    Export(ExportError),
    /// A bead this repo does not own: it lacks the prefix, or a longer
    /// configured prefix claims it. bd may have read another repo's
    /// database, and that export would mark every note removed.
    Foreign {
        id: String,
        prefix: String,
    },
    Scan(ScanError),
    Note(write::Error),
}

impl Error {
    pub fn stage(&self) -> Stage {
        match self {
            Error::Settings(_) => Stage::Config,
            Error::NoVault(_) | Error::Scan(_) => Stage::Vault,
            Error::Export(_) | Error::Foreign { .. } => Stage::Export,
            Error::Note(_) => Stage::Write,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Settings(e) => e.fmt(f),
            Error::NoVault(path) => write!(f, "{}: vault is not a directory", path.display()),
            Error::Export(e) => e.fmt(f),
            Error::Foreign { id, prefix } => write!(
                f,
                "bd export: {id} is not a \"{prefix}-\" bead; refusing the export"
            ),
            Error::Scan(e) => e.fmt(f),
            Error::Note(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Settings(e) => Some(e),
            Error::Export(e) => Some(e),
            Error::Scan(e) => Some(e),
            Error::Note(e) => Some(e),
            Error::NoVault(_) | Error::Foreign { .. } => None,
        }
    }
}

/// Holding `_lock` is the caller's proof that no other run reads or
/// writes between this run's export and its last write.
pub fn sync_repo(_lock: &Lock, req: &Request) -> Result<Report, Error> {
    let mut report = Report::default();
    let settings = repo::settings(req.bd, req.repo).map_err(Error::Settings)?;
    if !req.vault.is_dir() {
        return Err(Error::NoVault(req.vault.to_path_buf()));
    }
    let filer = match req.claims {
        Some(claims) => Filer::Field(claims),
        None => match project::map(req.vault, req.repo, &settings) {
            Ok(project) => Filer::Repo(project),
            Err(unmapped) => {
                report.warnings.push(Warning::Unmapped(unmapped));
                return Ok(report);
            }
        },
    };
    let prefix = settings.prefix;
    if !req.scope.prefixes.contains(&prefix) {
        report.warnings.push(Warning::UncheckedPrefix {
            prefix: prefix.clone(),
        });
    }
    let field = req.claims.map(|claims| &claims.field);
    let mut export =
        export::export_with_field(req.bd, &req.repo.root, field).map_err(Error::Export)?;
    let mut prefixes: Vec<&str> = req.scope.prefixes.iter().map(String::as_str).collect();
    prefixes.push(&prefix);
    let foreign = export
        .beads
        .iter()
        .chain(&export.scaffolding)
        .find(|b| owner(&b.id, &prefixes) != Some(prefix.as_str()));
    if let Some(bead) = foreign {
        return Err(Error::Foreign {
            id: bead.id.clone(),
            prefix,
        });
    }
    let fields = std::mem::take(&mut export.fields);
    let graph = Graph::new(export);
    let filing = match filer {
        Filer::Repo(project) => Filing::Repo(project),
        Filer::Field(claims) => {
            Filing::Claims(claim_beads(req, claims, &graph, &fields, &mut report))
        }
    };
    let index = Index::scan(req.vault).map_err(Error::Scan)?;
    // A folder no run writes to again would keep its leftover, and
    // Syncthing would copy it to every device.
    for rel in index.stale() {
        let _ = fs::remove_file(req.vault.join(rel));
    }
    let links = Links::new(&index, hidden(req.vault, &filing, &graph, &index));
    for bead in graph.notes() {
        let Some(project) = filing.project(&bead.id) else {
            continue;
        };
        let cx = Context {
            graph: &graph,
            links: &links,
            project: &project.name,
            prefix: &prefix,
            tz: req.tz,
        };
        let text = render::note(bead, &cx);
        match index.generated(&bead.id) {
            Generated::One(note) => {
                let path = req.vault.join(&note.path);
                let old = fs::read(&path).map_err(|source| {
                    Error::Note(write::Error::Io {
                        path: path.clone(),
                        source,
                    })
                })?;
                if old == text.as_bytes() {
                    report.unchanged += 1;
                } else {
                    write::replace(&path, &text).map_err(Error::Note)?;
                    report.updated.push(note.path.clone());
                }
            }
            Generated::Missing => {
                let rel = project
                    .folder()
                    .join(filename::filename(&bead.title, &bead.id));
                match write::create(&req.vault.join(&rel), &text) {
                    Ok(()) => report.created.push(rel),
                    Err(write::Error::Taken(_)) => report.warnings.push(Warning::Taken {
                        id: bead.id.clone(),
                        path: rel,
                    }),
                    Err(e) => return Err(Error::Note(e)),
                }
            }
            Generated::Duplicate(notes) => report.warnings.push(Warning::Duplicate {
                id: bead.id.clone(),
                paths: notes.iter().map(|n| n.path.clone()).collect(),
            }),
        }
    }
    if let Some(claims) = req.claims
        && !req.degraded
    {
        write_index(req, &prefix, &claims.field, &mut report)?;
    }
    if req.claims.is_some() && req.degraded {
        report.warnings.push(Warning::Degraded);
        return Ok(report);
    }
    if !req.scope.unreadable.is_empty() {
        report.warnings.push(Warning::ScopeIncomplete {
            repos: req.scope.unreadable.clone(),
        });
        return Ok(report);
    }
    mark_removed(
        req.vault,
        &req.scope.prefixes,
        &prefix,
        &graph,
        &index,
        &mut report,
    )?;
    Ok(report)
}

/// A degraded run must not call this: its unclaimed list may hold beads
/// an unresolvable entry would claim.
fn write_index(
    req: &Request,
    prefix: &str,
    field: &FieldPath,
    report: &mut Report,
) -> Result<(), Error> {
    if !req.vault.join(req.unfiled).is_dir() {
        report.conditions.push(Condition::UnfiledMissing {
            folder: req.unfiled.into(),
        });
        return Ok(());
    }
    let rel = req.unfiled.join(unfiled::file_name(prefix));
    let path = req.vault.join(&rel);
    let text = unfiled::index(prefix, field, &report.unclaimed);
    let io_err = |source| {
        Error::Note(write::Error::Io {
            path: path.clone(),
            source,
        })
    };
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => match write::create(&path, &text) {
            Ok(()) => report.created.push(rel),
            Err(write::Error::Taken(_)) => {
                report.conditions.push(Condition::IndexTaken { path: rel })
            }
            Err(e) => return Err(Error::Note(e)),
        },
        Err(e) => return Err(io_err(e)),
        Ok(_) if vault::own_index(&path, prefix).map_err(io_err)? => {
            if fs::read(&path).map_err(io_err)? == text.as_bytes() {
                report.unchanged += 1;
            } else {
                write::replace(&path, &text).map_err(Error::Note)?;
                report.updated.push(rel);
            }
        }
        Ok(_) => report.conditions.push(Condition::IndexTaken { path: rel }),
    }
    Ok(())
}

/// Who decides where this run files notes, known before the export.
enum Filer<'a> {
    Repo(Project),
    Field(&'a FieldClaims),
}

enum Filing {
    Repo(Project),
    /// Only claimed beads appear; the rest get no note.
    Claims(HashMap<String, Project>),
}

impl Filing {
    fn project(&self, id: &str) -> Option<&Project> {
        match self {
            Filing::Repo(project) => Some(project),
            Filing::Claims(filed) => filed.get(id),
        }
    }
}

fn claim_beads(
    req: &Request,
    claims: &FieldClaims,
    graph: &Graph,
    fields: &HashMap<String, FieldValue>,
    report: &mut Report,
) -> HashMap<String, Project> {
    let partition = claim::partition(req.vault, graph.notes(), fields, &claims.claims);
    report
        .conditions
        .extend(claim::conditions(&partition.unclaimed));
    report.unclaimed = partition.unclaimed;
    report.degraded = req.degraded;
    partition.filed
}

/// Beads without a note that links must show by id alone: their new
/// note's filename is held by a file callisto did not write, or this
/// run writes no note for them.
fn hidden(vault: &Path, filing: &Filing, graph: &Graph, index: &Index) -> HashSet<String> {
    graph
        .notes()
        .iter()
        .filter(|b| index.generated(&b.id) == Generated::Missing)
        .filter(|b| match filing.project(&b.id) {
            Some(project) => {
                let rel = project.folder().join(filename::filename(&b.title, &b.id));
                fs::symlink_metadata(vault.join(rel)).is_ok()
            }
            None => true,
        })
        .map(|b| b.id.clone())
        .collect()
}

/// Marks the repo's notes whose beads no longer get a note. A bead that
/// turned into scaffolding counts as removed: nothing renders it now.
fn mark_removed(
    vault: &Path,
    configured: &[String],
    prefix: &str,
    graph: &Graph,
    index: &Index,
    report: &mut Report,
) -> Result<(), Error> {
    let configured: Vec<&str> = configured.iter().map(String::as_str).collect();
    let mut prefixes = configured.clone();
    prefixes.push(prefix);
    let mut seen = HashSet::new();
    for note in index.repo_notes(prefix, &configured) {
        for id in &note.beads {
            if !note.owned_by(id, prefix, &prefixes)
                || graph.note(id).is_some()
                || !seen.insert(id.as_str())
            {
                continue;
            }
            match index.generated(id) {
                Generated::One(found) => {
                    let path = vault.join(&found.path);
                    let old = fs::read(&path).map_err(|source| {
                        Error::Note(write::Error::Io {
                            path: path.clone(),
                            source,
                        })
                    })?;
                    let Ok(old) = String::from_utf8(old) else {
                        report.warnings.push(Warning::NotUtf8 {
                            id: id.clone(),
                            path: found.path.clone(),
                        });
                        continue;
                    };
                    if let Some(text) = removed::mark(&old) {
                        write::replace(&path, &text).map_err(Error::Note)?;
                        report.removed.push(found.path.clone());
                    }
                }
                Generated::Duplicate(notes) => report.warnings.push(Warning::Duplicate {
                    id: id.clone(),
                    paths: notes.iter().map(|n| n.path.clone()).collect(),
                }),
                Generated::Missing => {}
            }
        }
    }
    Ok(())
}
