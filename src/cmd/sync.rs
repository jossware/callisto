//! `callisto sync`: mirror the databases of the given repos, or every
//! configured database.

use std::fmt::Display;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jiff::tz::TimeZone;

use crate::bd::Bd;
use crate::config::{self, Config, VaultError};
use crate::database::{self, Database, Databases, Target};
use crate::errlog::{self, Entry, Stage};
use crate::lock::{self, Lock};
use crate::paths::{self, Env};
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

    fn report(&mut self, database: &Database, report: &Report) {
        for warning in &report.warnings {
            let _ = writeln!(
                self.err,
                "callisto: {}: warning: {warning}",
                database.dir.display()
            );
        }
        let _ = writeln!(
            self.out,
            "{}: {} created, {} updated, {} removed, {} unchanged",
            database.name,
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
    if args.all && config.databases.is_empty() {
        let error = format!("sync --all: no databases configured in {}", config_file());
        return out.fail(Stage::Config, None, error);
    }
    let lock = match lock::acquire(&paths::sync_lock(state)) {
        Ok(lock) => lock,
        Err(e) => return out.fail(Stage::Lock, None, e),
    };
    let mut databases = Databases::new(&config.databases);
    let targets = if args.all {
        let (pulled, missing) = pull(bd, &databases, out);
        // With nothing pulled there is nothing to sync, so no prefix is
        // worth reading.
        if pulled.is_empty() {
            let summaries = [standing::config_summary(missing)];
            return finish(&lock, state, &summaries, out);
        }
        // After the pulls, so the prefixes are read from pulled data.
        databases.read_prefixes(bd);
        pulled
            .into_iter()
            .filter(|d| databases.prefix(&d.dir).is_some())
            .collect()
    } else {
        let paths: Vec<PathBuf> = if args.repos.is_empty() {
            vec![cwd.to_path_buf()]
        } else {
            args.repos.iter().map(|p| cwd.join(p)).collect()
        };
        let repos = resolve(&paths, out);
        if repos.is_empty() {
            return;
        }
        let targets = find(bd, &databases, &repos, out);
        databases.read_prefixes(bd);
        targets
    };
    let mut summaries = vec![standing::config_summary(databases.conditions())];
    let scope = databases.scope();
    let tz = TimeZone::system();
    for database in &targets {
        let req = Request {
            bd,
            vault: &vault,
            database,
            configured: databases.configured(&database.dir).is_some(),
            scope: &scope,
            tz: &tz,
            unfiled: &config.unfiled,
            locations: &config.locations,
        };
        match sync::sync_database(&lock, &req) {
            Ok(report) => {
                out.report(database, &report);
                summaries.push(Summary {
                    subject: Subject::Database(database.dir.clone()),
                    conditions: report.conditions,
                });
            }
            Err(e) => out.fail(e.stage(), Some(&database.dir), e),
        }
    }
    finish(&lock, state, &summaries, out);
}

/// The databases `repos` use, each once, in order.
fn find(bd: &Bd, databases: &Databases, repos: &[Repo], out: &mut Out) -> Vec<Database> {
    let mut found: Vec<Database> = Vec::new();
    for repo in repos {
        let database = match database::find(bd, databases, repo) {
            Ok(Target::Configured(database) | Target::Local(database)) => database,
            Ok(Target::Unconfigured(dir)) => {
                let error = format!("database {} is not configured", dir.display());
                out.fail(Stage::Config, Some(&repo.root), error);
                continue;
            }
            Err(e) => {
                out.fail(Stage::Config, Some(&repo.root), e);
                continue;
            }
        };
        if !found.iter().any(|d| d.dir == database.dir) {
            found.push(database);
        }
    }
    found
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

/// Pulls each configured database whose directory exists. Returns the
/// pulled databases, and a condition for each missing directory.
fn pull(bd: &Bd, databases: &Databases, out: &mut Out) -> (Vec<Database>, Vec<Condition>) {
    let mut pulled = Vec::new();
    let mut missing = Vec::new();
    for database in databases.all() {
        if !database.dir.is_dir() {
            missing.push(Condition::Unreadable {
                database: database.dir.clone(),
                reason: "no such directory".to_string(),
            });
            continue;
        }
        match bd.run_on(&database.dir, &["dolt", "pull"]) {
            Ok(_) => pulled.push(database.clone()),
            Err(e) => out.fail(Stage::Pull, Some(&database.dir), e),
        }
    }
    (pulled, missing)
}

/// The repos `paths` name, each main checkout once, in order.
fn resolve(paths: &[PathBuf], out: &mut Out) -> Vec<Repo> {
    let mut repos: Vec<Repo> = Vec::new();
    for path in paths {
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
        if !repos.iter().any(|r| r.root == repo.root) {
            repos.push(repo);
        }
    }
    repos
}
