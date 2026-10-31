//! `callisto hook`: sync the repo a harness Stop hook names on stdin.
//!
//! A hook MUST NOT disturb the harness. Every path exits 0, stdout stays
//! empty, and failures go to the error log only. Stream writes ignore
//! errors: `eprintln!` panics on a closed stream.

use std::io::{self, Read, Write};
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Mutex, TryLockError};

use jiff::tz::TimeZone;
use serde_json::Value;

use crate::bd::Bd;
use crate::config::{self, Config, VaultError};
use crate::errlog::{self, Entry, Stage};
use crate::lock;
use crate::paths::{self, Env};
use crate::repo;
use crate::sync::{self, Request};

/// The repo being synced: the panic hook's only view of the run.
static REPO: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The payload's `cwd`. Claude Code and Codex both send one; nothing
/// else in the payload matters here.
fn read_payload(input: &[u8]) -> Result<PathBuf, String> {
    let value: Value =
        serde_json::from_slice(input).map_err(|e| format!("payload is not JSON: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "payload is not a JSON object".to_string())?;
    let cwd = object
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "payload has no cwd".to_string())?;
    let cwd = PathBuf::from(cwd);
    // The process's own cwd is the harness's choice, not the session's.
    if !cwd.is_absolute() {
        return Err(format!("payload cwd is not absolute: {}", cwd.display()));
    }
    Ok(cwd)
}

pub fn run(vault: Option<&Path>, env: &Env, bd: &Bd, stdin: &mut dyn Read, err: &mut dyn Write) {
    let mut input = Vec::new();
    if let Err(e) = stdin.read_to_end(&mut input) {
        return fail(
            env,
            None,
            Stage::Payload,
            format!("couldn't read stdin: {e}"),
            err,
        );
    }
    let cwd = match read_payload(&input) {
        Ok(cwd) => cwd,
        Err(e) => return fail(env, None, Stage::Payload, e, err),
    };
    // Before the config, so a repo without beads costs nothing.
    let Some(repo) = repo::locate(&cwd).filter(|r| r.has_beads()) else {
        return;
    };
    let root = Some(repo.root.as_path());
    *REPO.lock().unwrap_or_else(|e| e.into_inner()) = Some(repo.root.clone());

    #[cfg(debug_assertions)]
    if std::env::var_os("CALLISTO_TEST_PANIC").is_some() {
        panic!("CALLISTO_TEST_PANIC is set");
    }

    let home = env.home.as_deref();
    let config = match &env.config {
        Some(dir) => match Config::load(&paths::config_file(dir), home) {
            Ok(config) => config,
            Err(e) => return fail(env, root, Stage::Config, e.to_string(), err),
        },
        None => Config::default(),
    };
    let vault = match config::vault(vault, &config, home) {
        Ok(vault) => vault,
        // A host with no vault set is not the designated host.
        Err(VaultError::Unset) => return,
        Err(e) => return fail(env, root, Stage::Vault, e.to_string(), err),
    };
    if !vault.is_dir() {
        return;
    }
    let Some(state) = &env.state else {
        return fail(env, root, Stage::Lock, "HOME is not set".into(), err);
    };
    // A held lock is a scheduled sync, which will write the same notes.
    let lock = match lock::try_acquire(&paths::sync_lock(state)) {
        Ok(lock) => lock,
        Err(lock::Error::Locked) => return,
        Err(e) => return fail(env, root, Stage::Lock, e.to_string(), err),
    };
    let scope = sync::scope(bd, &config.repos);
    let req = Request {
        bd,
        vault: &vault,
        repo: &repo,
        scope: &scope,
        tz: &TimeZone::system(),
    };
    // Warnings are dropped: they recur every turn, and `sync` shows them.
    if let Err(e) = sync::sync_repo(&lock, &req) {
        fail(env, root, e.stage(), e.to_string(), err);
    }
}

/// `hook` with arguments clap rejected. The harness settings own the
/// arguments, so a bad one is a payload failure, not a reason to fail
/// the harness.
pub fn bad_args(message: &str, env: &Env, err: &mut dyn Write) {
    fail(env, None, Stage::Payload, message.to_string(), err);
}

/// Logs a panic as a `panic` failure, then exits 0. Nothing in here may
/// panic: std aborts on a panic raised inside a panic hook.
pub fn install_panic_hook(env: Env) {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        default(info);
        let repo = match REPO.try_lock() {
            Ok(repo) => repo.clone(),
            Err(TryLockError::Poisoned(e)) => e.into_inner().clone(),
            Err(TryLockError::WouldBlock) => None,
        };
        fail(
            &env,
            repo.as_deref(),
            Stage::Panic,
            panic_message(info),
            &mut io::stderr(),
        );
        process::exit(0);
    }));
}

fn panic_message(info: &PanicHookInfo) -> String {
    let message = info.payload_as_str().unwrap_or("panic");
    match info.location() {
        Some(at) => format!("{message} at {}:{}", at.file(), at.line()),
        None => message.to_string(),
    }
}

/// Logs a failure. Without a state dir there is no log, so it goes to
/// stderr, which the harness keeps out of the session.
pub fn fail(env: &Env, repo: Option<&Path>, stage: Stage, error: String, err: &mut dyn Write) {
    let entry = Entry {
        command: Some("hook".into()),
        repo: repo.map(Path::to_path_buf),
        ..Entry::new(stage, error)
    };
    match &env.state {
        Some(state) => errlog::record(&paths::error_log(state), &entry),
        None => {
            let _ = writeln!(err, "callisto: hook: {}", entry.error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_and_codex_payloads() {
        let claude = br#"{"session_id":"s","transcript_path":"/t.jsonl","cwd":"/x/y","hook_event_name":"Stop","stop_hook_active":false}"#;
        let codex = br#"{"session_id":"s","turn_id":"t","transcript_path":null,"cwd":"/x/y","hook_event_name":"Stop","model":"gpt-5","stop_hook_active":false,"last_assistant_message":"done"}"#;
        for input in [&claude[..], codex] {
            assert_eq!(read_payload(input), Ok(PathBuf::from("/x/y")));
        }
    }

    #[test]
    fn rejects_unusable_payloads() {
        for (input, why) in [
            (&b"not json"[..], "payload is not JSON"),
            (b"", "payload is not JSON"),
            (b"[1]", "payload is not a JSON object"),
            (b"{}", "payload has no cwd"),
            (br#"{"cwd":""}"#, "payload has no cwd"),
            (br#"{"cwd":7}"#, "payload has no cwd"),
            (br#"{"cwd":"rel/dir"}"#, "payload cwd is not absolute"),
        ] {
            let e = read_payload(input).unwrap_err();
            assert!(e.starts_with(why), "{e:?} for {input:?}");
        }
    }

    #[test]
    fn fails_to_stderr_without_a_state_dir() {
        let env = Env {
            home: None,
            config: None,
            state: None,
        };
        let mut err = Vec::new();
        fail(&env, None, Stage::Payload, "boom".into(), &mut err);
        assert_eq!(String::from_utf8(err).unwrap(), "callisto: hook: boom\n");
    }
}
