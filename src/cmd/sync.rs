//! `callisto sync`: mirror the given repos, or every configured one.

use std::fmt::Display;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jiff::tz::TimeZone;

use crate::bd::Bd;
use crate::config::{self, Config, RepoEntry, VaultError};
use crate::errlog::{self, Entry, Stage};
use crate::group::{FieldClaims, Group, Groups, Ownership};
use crate::lock::{self, Lock};
use crate::paths::{self, Env};
use crate::remote::Git;
use crate::repo::{self, Repo};
use crate::standing::{self, Condition, Subject, Summary};
use crate::sync::{self, Report, Request};

pub struct Args<'a> {
    pub repos: &'a [PathBuf],
    pub all: bool,
    pub vault: Option<&'a Path>,
}

/// Prints failures to stderr and records them in the error log. Stream
/// writes ignore errors: `eprintln!` panics on a closed stream.
struct Out<'a> {
    log: PathBuf,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    failed: bool,
}

impl Out<'_> {
    fn fail(&mut self, stage: Stage, repo: Option<&Path>, error: impl Display) {
        self.failed = true;
        let error = error.to_string();
        let _ = match repo {
            Some(repo) => writeln!(self.err, "callisto: {}: {error}", repo.display()),
            None => writeln!(self.err, "callisto: {error}"),
        };
        let entry = Entry {
            command: Some("sync".into()),
            repo: repo.map(Path::to_path_buf),
            ..Entry::new(stage, error)
        };
        errlog::record(&self.log, &entry);
    }

    fn report(&mut self, repo: &Repo, report: &Report) {
        for warning in &report.warnings {
            let _ = writeln!(
                self.err,
                "callisto: {}: warning: {warning}",
                repo.root.display()
            );
        }
        let _ = writeln!(
            self.out,
            "{}: {} created, {} updated, {} removed, {} unchanged",
            repo.name,
            report.created.len(),
            report.updated.len(),
            report.removed.len(),
            report.unchanged
        );
    }
}

pub fn run(
    args: &Args,
    env: &Env,
    bd: &Bd,
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ExitCode {
    let Some(state) = &env.state else {
        let _ = writeln!(err, "callisto: HOME is not set");
        return ExitCode::FAILURE;
    };
    let mut out = Out {
        log: paths::error_log(state),
        out,
        err,
        failed: false,
    };
    sync(args, env, bd, cwd, state, &mut out);
    if out.failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn sync(args: &Args, env: &Env, bd: &Bd, cwd: &Path, state: &Path, out: &mut Out) {
    let home = env.home.as_deref();
    let config = match &env.config {
        Some(dir) => match Config::load(&paths::config_file(dir), home) {
            Ok(config) => config,
            Err(e) => return out.fail(Stage::Config, None, e),
        },
        None => Config::default(),
    };
    let config_file = || {
        env.config.as_deref().map_or_else(
            || "the config file".to_string(),
            |dir| paths::config_file(dir).display().to_string(),
        )
    };
    let vault = match config::vault(args.vault, &config, home) {
        Ok(vault) => vault,
        Err(VaultError::Unset) => {
            let error = format!(
                "no vault set: pass --vault or set vault in {}",
                config_file()
            );
            return out.fail(Stage::Vault, None, error);
        }
        Err(e) => return out.fail(Stage::Vault, None, e),
    };
    if !vault.is_dir() {
        let error = format!("{}: vault is not a directory", vault.display());
        return out.fail(Stage::Vault, None, error);
    }
    if args.all && config.repos.is_empty() {
        let error = format!("sync --all: no repos configured in {}", config_file());
        return out.fail(Stage::Config, None, error);
    }
    let lock = match lock::acquire(&paths::sync_lock(state)) {
        Ok(lock) => lock,
        Err(e) => return out.fail(Stage::Lock, None, e),
    };
    let git = Git::default();
    let (repos, mut groups) = if args.all {
        let mut groups = Groups::derive(bd, &config.repos);
        let repos = resolve(&all_targets(&config, &groups), out);
        // With no prefix read yet no claim resolves, so this settles only
        // the rules that need no value: enough to skip a broken group's
        // pull.
        groups.resolve(&git);
        (repos, groups)
    } else {
        let targets: Vec<PathBuf> = if args.repos.is_empty() {
            vec![cwd.to_path_buf()]
        } else {
            args.repos.iter().map(|p| cwd.join(p)).collect()
        };
        let repos = resolve(&targets, out);
        if repos.is_empty() {
            return;
        }
        (repos, Groups::derive(bd, &config.repos))
    };
    // Before the prefixes, so they are read from pulled data.
    let pulled = args.all.then(|| pull(bd, &groups, &repos, out));
    let tables = config
        .repos
        .iter()
        .any(|e| matches!(e, RepoEntry::Table(_)));
    // With nothing pulled there is nothing to sync, but a table entry
    // still needs its group's prefix to resolve.
    if pulled.as_ref().is_none_or(|p| !p.is_empty()) || tables {
        groups.read_prefixes(bd);
    }
    groups.resolve(&git);
    let mut summaries = vec![standing::config_summary(groups.conditions())];
    let scope = groups.scope();
    let tz = TimeZone::system();
    let mut sync_one = |repo: &Repo, group: Option<&Group>, listed: bool, out: &mut Out| {
        let subject = match group {
            Some(group) => Subject::group(group),
            None => Subject::Checkout(repo.root.clone()),
        };
        let (claims, degraded) = match plan(&groups, group, listed) {
            Plan::Sync { claims, degraded } => (claims, degraded),
            Plan::Skip(condition) => {
                return summaries.push(Summary {
                    subject,
                    conditions: vec![condition],
                });
            }
        };
        let req = Request {
            bd,
            vault: &vault,
            repo,
            scope: &scope,
            tz: &tz,
            claims,
            degraded,
        };
        match sync::sync_repo(&lock, &req) {
            Ok(report) => {
                out.report(repo, &report);
                summaries.push(Summary {
                    subject,
                    conditions: report.conditions,
                });
            }
            Err(e) => out.fail(e.stage(), Some(&repo.root), e),
        }
    };
    if let Some(pulled) = &pulled {
        for repo in pulled {
            sync_one(repo, groups.of_member(&repo.root), true, out);
        }
    } else {
        let mut synced: Vec<&Path> = Vec::new();
        for target in &repos {
            match groups.of_target(bd, target) {
                Ok(Some(group)) => {
                    if synced.contains(&group.database.as_path()) {
                        continue;
                    }
                    synced.push(&group.database);
                    sync_one(group.representative(Some(target)), Some(group), true, out);
                }
                Ok(None) => sync_one(target, None, false, out),
                Err(e) => out.fail(Stage::Config, Some(&target.root), e),
            }
        }
    }
    finish(&lock, state, &summaries, out);
}

pub(crate) enum Plan<'a> {
    Sync {
        claims: Option<&'a FieldClaims>,
        degraded: bool,
    },
    /// Not synced; the condition says why.
    Skip(Condition),
}

/// How a run treats a target in `group`, or in no group. `listed` says
/// whether the target is a configured repo.
pub(crate) fn plan<'a>(groups: &'a Groups, group: Option<&'a Group>, listed: bool) -> Plan<'a> {
    match group.map(|g| &g.ownership) {
        Some(Ownership::Broken(reason)) => Plan::Skip(Condition::GroupRule {
            reason: reason.clone(),
        }),
        Some(Ownership::Field(claims)) => Plan::Sync {
            claims: Some(claims),
            degraded: groups.degraded(),
        },
        // An unresolvable table entry may be this checkout, whose beads
        // it would then file under the wrong project.
        None if !listed && groups.degraded() => Plan::Skip(Condition::UnlistedSkipped),
        Some(Ownership::Prefix) | None => Plan::Sync {
            claims: None,
            degraded: false,
        },
    }
}

/// The configured paths, less the table entries `groups` could not
/// place: their unresolvable condition is their only report.
fn all_targets(config: &Config, groups: &Groups) -> Vec<PathBuf> {
    config
        .repos
        .iter()
        .filter(|entry| match entry {
            RepoEntry::Path(_) => true,
            RepoEntry::Table(table) => {
                repo::locate(&table.path).is_some_and(|repo| groups.of_member(&repo.root).is_some())
            }
        })
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

/// Prints and records the run's summaries while the caller still holds
/// the lock.
fn finish(lock: &Lock, state: &Path, summaries: &[Summary], out: &mut Out) {
    for summary in summaries.iter().filter(|s| !s.conditions.is_empty()) {
        let _ = writeln!(
            out.err,
            "callisto: {}: {}",
            summary.subject.label(),
            summary.text()
        );
    }
    standing::record(lock, state, "sync", summaries);
    if standing::any_fails(summaries) {
        out.failed = true;
    }
}

/// Reports the resolved repos `groups` could not place, then pulls each
/// group once, in its first resolved member. Returns the repos to sync,
/// including an unpulled member of each broken group so its rule break
/// is reported.
fn pull(bd: &Bd, groups: &Groups, repos: &[Repo], out: &mut Out) -> Vec<Repo> {
    let resolved = |root: &Path| repos.iter().any(|r| r.root == root);
    let errored = |root: &Path| {
        groups
            .ungrouped
            .iter()
            .any(|u| u.error.is_some() && u.repo.as_ref().is_some_and(|r| r.root == root))
    };
    for ungrouped in &groups.ungrouped {
        if let (Some(repo), Some(e)) = (&ungrouped.repo, &ungrouped.error)
            && resolved(&repo.root)
        {
            out.fail(Stage::Config, Some(&repo.root), e);
        }
    }
    let mut pulled = Vec::new();
    let mut tried: Vec<&Path> = Vec::new();
    for group in &groups.groups {
        let Some(member) = group.members.iter().find(|m| resolved(&m.root)) else {
            continue;
        };
        tried.push(&group.database);
        if broken(group) || pull_in(bd, member, out) {
            pulled.push(member.clone());
        }
    }
    // Grouping locates configured paths without following symlinks, so a
    // resolved repo can be one it never placed.
    for repo in repos {
        if groups.of_member(&repo.root).is_some() || errored(&repo.root) {
            continue;
        }
        let (synced, unpulled) = match groups.of_target(bd, repo) {
            Ok(Some(group)) => {
                if tried.contains(&group.database.as_path()) {
                    continue;
                }
                tried.push(&group.database);
                (group.representative(Some(repo)), broken(group))
            }
            Ok(None) => (repo, false),
            Err(e) => {
                out.fail(Stage::Config, Some(&repo.root), e);
                continue;
            }
        };
        if unpulled || pull_in(bd, repo, out) {
            pulled.push(synced.clone());
        }
    }
    pulled
}

fn broken(group: &Group) -> bool {
    matches!(group.ownership, Ownership::Broken(_))
}

fn pull_in(bd: &Bd, repo: &Repo, out: &mut Out) -> bool {
    match bd.run(&repo.root, &["dolt", "pull"]) {
        Ok(_) => true,
        Err(e) => {
            out.fail(Stage::Pull, Some(&repo.root), e);
            false
        }
    }
}

/// The repos `targets` name, each main checkout once, in order.
fn resolve(targets: &[PathBuf], out: &mut Out) -> Vec<Repo> {
    let mut repos: Vec<Repo> = Vec::new();
    for path in targets {
        // locate walks lexical ancestors and accepts missing paths, so a
        // mistyped `typo` or `../notes` would land on the enclosing repo.
        let real = match path.canonicalize() {
            Ok(real) => real,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                out.fail(Stage::Config, Some(path), "no such directory");
                continue;
            }
            Err(e) => {
                out.fail(Stage::Config, Some(path), e);
                continue;
            }
        };
        let Some(repo) = repo::locate(&real) else {
            out.fail(Stage::Config, Some(path), "not in a repository");
            continue;
        };
        if !repo.has_beads() {
            out.fail(Stage::Config, Some(&repo.root), "no .beads directory");
        } else if !repos.iter().any(|r| r.root == repo.root) {
            repos.push(repo);
        }
    }
    repos
}
