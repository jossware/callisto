//! End-to-end tests against the real binary, with HOME, the XDG dirs and
//! PATH inside a temp dir so `bd` is always the stand-in below.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

mod common;

const BIN: &str = env!("CARGO_BIN_EXE_callisto");

// Shell builtins only: PATH holds nothing but this script, as in the
// Linux Nix build sandbox.
const FAKE_BD: &str = r#"#!/bin/sh
echo "$*" >> .beads/calls
case "$1" in
export)
  [ -e .beads/export-fails ] && { echo 'no database' >&2; exit 1; }
  while IFS= read -r l; do printf '%s\n' "$l"; done < .beads/export.jsonl ;;
config) case "$4" in
  issue_prefix) read -r p < .beads/prefix
    printf '{"key":"issue_prefix","value":"%s"}\n' "$p" ;;
  *) printf '{"key":"%s","value":""}\n' "$4" ;;
  esac ;;
dolt)
  [ -e .beads/pull-fails ] && { echo 'no remote' >&2; exit 1; }
  exit 0 ;;
*) exit 9 ;;
esac
"#;

struct Sandbox {
    tmp: TempDir,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl From<Output> for Run {
    fn from(out: Output) -> Run {
        Run {
            code: out.status.code(),
            stdout: String::from_utf8(out.stdout).unwrap(),
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    }
}

fn line(id: &str, title: &str) -> String {
    format!(
        r#"{{"id":"{id}","title":"{title}","status":"open","priority":2,"created_at":"2026-10-11T02:21:06Z","updated_at":"2026-10-11T02:21:40Z"}}"#
    )
}

impl Sandbox {
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        for dir in ["home", "bin", "vault", "repos"] {
            fs::create_dir(tmp.path().join(dir)).unwrap();
        }
        let bd = tmp.path().join("bin/bd");
        common::write_script(&bd, FAKE_BD);
        Sandbox { tmp }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn vault(&self) -> PathBuf {
        self.path("vault")
    }

    fn project(&self, name: &str) {
        let dir = self.vault().join("Projects").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.md")), "# home\n").unwrap();
    }

    /// A checkout `repos/<name>` whose beads use `prefix`, with a vault
    /// project of the same name.
    fn repo(&self, name: &str, prefix: &str, beads: &[(&str, &str)]) -> PathBuf {
        let dir = self.path("repos").join(name);
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join(".beads")).unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join(".beads/prefix"), prefix).unwrap();
        let export: String = beads
            .iter()
            .map(|(id, title)| line(id, title) + "\n")
            .collect();
        fs::write(dir.join(".beads/export.jsonl"), export).unwrap();
        self.project(name);
        dir
    }

    fn flag(&self, name: &str, flag: &str) {
        fs::write(self.path("repos").join(name).join(".beads").join(flag), "").unwrap();
    }

    fn calls(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.path("repos").join(name).join(".beads/calls")).ok()
    }

    fn config(&self, repos: &[&str]) {
        let dir = self.path("config/callisto");
        fs::create_dir_all(&dir).unwrap();
        let list: Vec<String> = repos
            .iter()
            .map(|r| format!("{:?}", self.path("repos").join(r).display().to_string()))
            .collect();
        let text = format!(
            "vault = {:?}\nrepos = [{}]\n",
            self.vault().display().to_string(),
            list.join(", ")
        );
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    fn log(&self) -> Vec<Value> {
        fs::read_to_string(self.path("state/callisto/errors.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("PATH", self.path("bin"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Run {
        self.command(cwd, args).output().unwrap().into()
    }

    /// `hook` plus `args`, from the sandbox root, with `payload` on stdin.
    fn hook(&self, payload: &str, args: &[&str], envs: &[(&str, &str)]) -> Run {
        let mut all = vec!["hook"];
        all.extend_from_slice(args);
        self.piped(payload, &all, envs)
    }

    /// `args` as given, from the sandbox root, with `payload` on stdin.
    fn piped(&self, payload: &str, args: &[&str], envs: &[(&str, &str)]) -> Run {
        let mut cmd = self.command(self.tmp.path(), args);
        cmd.envs(envs.iter().copied()).stdin(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        // The hook may exit without reading stdin.
        let _ = stdin.write_all(payload.as_bytes());
        drop(stdin);
        child.wait_with_output().unwrap().into()
    }

    fn spawn(&self, cwd: &Path, args: &[&str]) -> Child {
        self.command(cwd, args).spawn().unwrap()
    }

    /// `sync` with the vault passed by flag, so no config file is needed.
    fn sync(&self, cwd: &Path, args: &[&str]) -> Run {
        let vault = self.vault().display().to_string();
        let mut all = vec!["--vault", vault.as_str(), "sync"];
        all.extend_from_slice(args);
        self.run(cwd, &all)
    }

    fn note(&self, rel: &str) -> bool {
        self.vault().join(rel).is_file()
    }
}

fn ends_with(entry: &Value, key: &str, suffix: &str) -> bool {
    entry[key].as_str().is_some_and(|s| s.ends_with(suffix))
}

/// A Claude Code Stop payload.
fn payload(cwd: &Path) -> String {
    serde_json::json!({
        "session_id": "5f0c2a1e-7b3d-4c9a-9e21-3d4b5a6c7d8e",
        "transcript_path": "/h/.claude/projects/x/5f0c2a1e.jsonl",
        "cwd": cwd,
        "hook_event_name": "Stop",
        "stop_hook_active": false,
    })
    .to_string()
}

/// A Codex Stop payload.
fn codex_payload(cwd: &Path) -> String {
    serde_json::json!({
        "session_id": "01a0b000-0000-7000-8000-000000000001",
        "turn_id": "t1",
        "transcript_path": null,
        "cwd": cwd,
        "hook_event_name": "Stop",
        "model": "gpt-5",
        "stop_hook_active": false,
        "last_assistant_message": "done",
    })
    .to_string()
}

fn assert_quiet_exit(out: &Run) {
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn hook_syncs_the_cwd_repo() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    let out = sb.hook(&payload(&repo.join("sub")), &[], &[]);
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    assert!(sb.note("Projects/a/first - a-1.md"));
    assert!(sb.log().is_empty(), "{:?}", sb.log());
    let calls = sb.calls("a").unwrap();
    assert!(calls.contains("export"), "{calls}");
    assert!(!calls.contains("dolt"), "{calls}");
}

#[test]
fn hook_takes_a_codex_payload() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    let out = sb.hook(&codex_payload(&repo), &[], &[]);
    assert_quiet_exit(&out);
    assert!(sb.note("Projects/a/first - a-1.md"));
}

#[test]
fn hook_syncs_from_a_deleted_cwd() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    let out = sb.hook(&payload(&repo.join("gone/deeper")), &[], &[]);
    assert_quiet_exit(&out);
    assert!(sb.note("Projects/a/first - a-1.md"));
}

#[test]
fn hook_ignores_a_repo_without_beads() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.path("repos/plain/.git")).unwrap();
    for cwd in [sb.path("repos/plain"), sb.path("home")] {
        let out = sb.hook(&payload(&cwd), &[], &[]);
        assert_quiet_exit(&out);
        assert_eq!(out.stderr, "");
    }
    assert!(sb.log().is_empty(), "{:?}", sb.log());
}

#[test]
fn hook_ignores_a_missing_vault() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    let absent = sb.path("absent").display().to_string();
    let out = sb.hook(&payload(&repo), &["--vault", &absent], &[]);
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    assert_eq!(sb.calls("a"), None);
    assert!(sb.log().is_empty(), "{:?}", sb.log());
}

#[test]
fn hook_leaves_when_the_lock_is_held() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    let _held = callisto::lock::acquire(&sb.path("state/callisto/sync.lock")).unwrap();
    let out = sb.hook(&payload(&repo), &[], &[]);
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    assert_eq!(sb.calls("a"), None);
    assert!(sb.log().is_empty(), "{:?}", sb.log());
}

#[test]
fn hook_logs_a_failed_export() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    sb.flag("a", "export-fails");
    let out = sb.hook(&payload(&repo), &[], &[]);
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "export");
    assert_eq!(log[0]["command"], "hook");
    assert!(ends_with(&log[0], "repo", "repos/a"), "{log:?}");
    assert!(log[0]["error"].as_str().unwrap().contains("no database"));
}

#[test]
fn hook_logs_a_bad_payload() {
    let sb = Sandbox::new();
    let out = sb.hook("not json", &[], &[]);
    assert_quiet_exit(&out);
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "payload");
    assert_eq!(log[0]["command"], "hook");
}

#[test]
fn hook_logs_a_bad_config() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    fs::create_dir_all(sb.path("config/callisto")).unwrap();
    fs::write(sb.path("config/callisto/config.toml"), "repos = 7\n").unwrap();
    let out = sb.hook(&payload(&sb.path("home")), &[], &[]);
    assert_quiet_exit(&out);
    assert!(sb.log().is_empty(), "{:?}", sb.log());
    let out = sb.hook(&payload(&repo), &[], &[]);
    assert_quiet_exit(&out);
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "config");
    assert_eq!(sb.calls("a"), None);
}

#[test]
fn hook_is_silent_on_warnings() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    fs::remove_dir_all(sb.vault().join("Projects/a")).unwrap();
    let out = sb.hook(
        &payload(&repo),
        &["--vault", sb.vault().to_str().unwrap()],
        &[],
    );
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    assert!(sb.log().is_empty(), "{:?}", sb.log());
    assert!(sb.calls("a").is_some_and(|c| c.contains("config")));
}

#[test]
fn hook_takes_the_vault_before_the_subcommand() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    let vault = sb.vault().display().to_string();
    let out = sb.piped(&payload(&repo), &["--vault", &vault, "hook"], &[]);
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    assert!(sb.note("Projects/a/first - a-1.md"));
    assert!(sb.log().is_empty(), "{:?}", sb.log());
}

#[test]
fn hook_logs_an_unknown_root_flag() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    let out = sb.piped(&payload(&repo), &["--bogus", "hook"], &[]);
    assert_quiet_exit(&out);
    assert_eq!(sb.calls("a"), None);
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "payload");
    assert_eq!(log[0]["command"], "hook");
}

#[test]
fn default_is_the_cwd_repo() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    let out = sb.sync(&repo.join("sub"), &[]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(sb.note("Projects/a/first - a-1.md"));
    assert!(
        out.stdout
            .contains("a: 1 created, 0 updated, 0 removed, 0 unchanged"),
        "{}",
        out.stdout
    );
    assert!(!sb.calls("a").unwrap().contains("dolt"));
}

#[test]
fn explicit_relative_paths_sync() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    sb.repo("b", "b", &[("b-1", "second")]);
    let out = sb.sync(&sb.path("repos"), &["a", "b"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(sb.note("Projects/a/first - a-1.md"));
    assert!(sb.note("Projects/b/second - b-1.md"));
}

#[test]
fn bad_repo_does_not_stop_the_rest() {
    let sb = Sandbox::new();
    fs::create_dir(sb.path("repos/nowhere")).unwrap();
    sb.repo("a", "a", &[("a-1", "first")]);
    sb.repo("b", "b", &[("b-1", "second")]);
    sb.flag("b", "export-fails");
    let out = sb.sync(&sb.path("repos"), &["nowhere", "a", "b"]);
    assert_eq!(out.code, Some(1));
    assert!(sb.note("Projects/a/first - a-1.md"));
    assert!(out.stderr.contains("nowhere"), "{}", out.stderr);
    assert!(out.stderr.contains("no database"), "{}", out.stderr);
    let log = sb.log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log.iter().all(|e| e["command"] == "sync"), "{log:?}");
    assert!(
        log.iter()
            .any(|e| e["stage"] == "config" && ends_with(e, "repo", "nowhere")),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|e| e["stage"] == "export" && ends_with(e, "repo", "repos/b")),
        "{log:?}"
    );
}

#[test]
fn no_beads_is_an_error() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.path("repos/plain/.git")).unwrap();
    let out = sb.sync(&sb.path("repos/plain"), &[]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.contains("no .beads"), "{}", out.stderr);
}

#[test]
fn outside_any_repo_fails() {
    let sb = Sandbox::new();
    let out = sb.sync(&sb.path("home"), &[]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.contains("not in a repository"), "{}", out.stderr);
}

#[test]
fn missing_vault_runs_no_bd() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    let absent = sb.path("absent").display().to_string();
    let out = sb.run(&sb.path("repos"), &["--vault", &absent, "sync", "a"]);
    assert_eq!(out.code, Some(1));
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "vault");
    assert_eq!(sb.calls("a"), None);
}

#[test]
fn same_repo_twice_syncs_once() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    let out = sb.sync(&sb.path("repos"), &["a", "a/sub"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(out.stdout.matches("a: ").count(), 1, "{}", out.stdout);
    assert_eq!(sb.calls("a").unwrap().matches("export").count(), 1);
}

#[test]
fn unmapped_repo_warns_and_succeeds() {
    let sb = Sandbox::new();
    let repo = sb.repo("c", "c", &[("c-1", "first")]);
    fs::remove_dir_all(sb.vault().join("Projects/c")).unwrap();
    let out = sb.sync(&repo, &[]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(out.stderr.contains("warning:"), "{}", out.stderr);
    assert!(
        out.stderr.contains(r#"no project or area named "c""#),
        "{}",
        out.stderr
    );
    let root = repo.canonicalize().unwrap();
    let prefix = format!("callisto: {}: warning:", root.display());
    assert!(out.stderr.contains(&prefix), "{}", out.stderr);
    let root = root.display().to_string();
    assert_eq!(out.stderr.matches(&root).count(), 1, "{}", out.stderr);
}

#[test]
fn sync_names_the_root_once() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "", &[("a-1", "first")]);
    let out = sb.sync(&repo, &[]);
    assert_eq!(out.code, Some(1));
    let root = repo.canonicalize().unwrap().display().to_string();
    let line = format!("callisto: {root}: bd config has no issue_prefix");
    assert!(out.stderr.contains(&line), "{}", out.stderr);
    assert_eq!(out.stderr.matches(&root).count(), 1, "{}", out.stderr);
}

#[test]
fn explicit_sync_reads_the_configured_scope() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    let out = sb.sync(&repo, &[]);
    assert!(out.stderr.contains("unchecked"), "{}", out.stderr);
    sb.config(&["a"]);
    let out = sb.run(&repo, &["sync"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(!out.stderr.contains("unchecked"), "{}", out.stderr);
}

#[test]
fn all_skips_a_failed_pull() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    sb.repo("b", "b", &[("b-1", "second")]);
    sb.flag("b", "pull-fails");
    sb.config(&["a", "b"]);
    let out = sb.run(&sb.path("home"), &["sync", "--all"]);
    assert_eq!(out.code, Some(1));
    assert!(sb.note("Projects/a/first - a-1.md"));
    let b: Vec<_> = fs::read_dir(sb.vault().join("Projects/b"))
        .unwrap()
        .collect();
    assert_eq!(b.len(), 1);
    let calls = sb.calls("a").unwrap();
    let pull = calls.find("dolt pull").expect(&calls);
    assert!(pull < calls.find("export").expect(&calls), "{calls}");
    assert!(!sb.calls("b").unwrap().contains("export"));
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "pull");
    assert_eq!(log[0]["command"], "sync");
    assert!(ends_with(&log[0], "repo", "repos/b"), "{log:?}");
    assert!(log[0]["error"].as_str().unwrap().contains("no remote"));
    assert!(out.stderr.contains("no remote"), "{}", out.stderr);
}

#[test]
fn all_without_config_fails() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    let out = sb.sync(&sb.path("repos/a"), &["--all"]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.contains("no repos configured"), "{}", out.stderr);
    assert_eq!(sb.calls("a"), None);
}

#[test]
fn all_with_empty_list_fails() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&[]);
    let out = sb.run(&sb.path("repos/a"), &["sync", "--all"]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.contains("no repos configured"), "{}", out.stderr);
    assert_eq!(sb.calls("a"), None);
    assert_eq!(sb.log()[0]["stage"], "config");
}

#[test]
fn all_pulls_under_the_lock() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    let held = callisto::lock::acquire(&sb.path("state/callisto/sync.lock")).unwrap();
    let child = sb.spawn(&sb.path("home"), &["sync", "--all"]);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(sb.calls("a"), None);
    drop(held);
    let out: Run = child.wait_with_output().unwrap().into();
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(sb.calls("a").unwrap().starts_with("dolt pull"));
    assert!(sb.note("Projects/a/first - a-1.md"));
}

#[test]
fn plain_sibling_path_does_not_sync_the_cwd_repo() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    fs::create_dir(sb.path("repos/notes")).unwrap();
    let out = sb.sync(&repo, &["../notes"]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.contains("not in a repository"), "{}", out.stderr);
    assert_eq!(sb.calls("a"), None);
}

#[test]
fn missing_path_does_not_sync_the_cwd_repo() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    for path in ["typo", "../typo"] {
        let out = sb.sync(&repo, &[path]);
        assert_eq!(out.code, Some(1), "{path}");
        assert!(out.stderr.contains("no such directory"), "{}", out.stderr);
        assert_eq!(sb.calls("a"), None, "{path}");
    }
}

#[test]
fn hook_logs_bad_arguments() {
    let sb = Sandbox::new();
    let out = sb.hook("{}", &["extra"], &[]);
    assert_quiet_exit(&out);
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "payload");
    assert_eq!(log[0]["command"], "hook");
}

#[test]
fn hook_help_still_prints() {
    let sb = Sandbox::new();
    let out = sb.hook("", &["--help"], &[]);
    assert_eq!(out.code, Some(0));
    assert!(out.stdout.contains("Stop hook"), "{}", out.stdout);
    assert!(sb.log().is_empty(), "{:?}", sb.log());
}

// The panic trigger exists only in debug builds; nix builds release.
#[cfg(debug_assertions)]
#[test]
fn hook_logs_a_panic() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    let out = sb.hook(&payload(&repo), &[], &[("CALLISTO_TEST_PANIC", "1")]);
    assert_quiet_exit(&out);
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "panic");
    assert_eq!(log[0]["command"], "hook");
    assert!(ends_with(&log[0], "repo", "repos/a"), "{log:?}");
}

#[test]
fn sync_still_rejects_bad_arguments() {
    let sb = Sandbox::new();
    let out = sb.run(sb.tmp.path(), &["sync", "--bogus"]);
    assert_eq!(out.code, Some(2));
}

#[test]
fn no_scope_without_a_repo_to_sync() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    sb.config(&["a"]);
    fs::create_dir(sb.path("repos/notes")).unwrap();
    let out = sb.run(&sb.path("repos"), &["sync", "notes"]);
    assert_eq!(out.code, Some(1));
    assert_eq!(sb.calls("a"), None);
    sb.flag("a", "pull-fails");
    let out = sb.run(&sb.path("home"), &["sync", "--all"]);
    assert_eq!(out.code, Some(1));
    let calls = sb.calls("a").unwrap();
    assert!(!calls.contains("config"), "{calls}");
}

#[test]
fn hook_without_home_is_silent() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    // Without XDG_STATE_HOME as well, there is no log to fall back on.
    for envs in [&[("HOME", "")][..], &[("HOME", ""), ("XDG_STATE_HOME", "")]] {
        let out = sb.hook(&payload(&repo), &[], envs);
        assert_quiet_exit(&out);
        assert_eq!(out.stderr, "");
    }
    assert_eq!(sb.calls("a"), None);
    assert!(sb.log().is_empty(), "{:?}", sb.log());
}

#[test]
fn hook_logs_a_named_vault_it_cannot_resolve() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    let out = sb.hook(&payload(&repo), &["--vault", "~/org"], &[("HOME", "")]);
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    assert_eq!(sb.calls("a"), None);
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "vault");
    assert_eq!(log[0]["command"], "hook");
    assert!(ends_with(&log[0], "repo", "repos/a"), "{log:?}");
}

#[test]
fn sync_without_a_vault_runs_no_bd() {
    let sb = Sandbox::new();
    sb.repo("a", "a", &[("a-1", "first")]);
    let out = sb.run(&sb.path("repos"), &["sync", "a"]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.contains("--vault"), "{}", out.stderr);
    assert!(out.stderr.contains("config.toml"), "{}", out.stderr);
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "vault");
    assert_eq!(sb.calls("a"), None);
}

#[test]
fn sync_all_without_a_vault_fails_at_vault() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    let dir = sb.path("config/callisto");
    fs::create_dir_all(&dir).unwrap();
    let text = format!("repos = [{:?}]\n", repo.display().to_string());
    fs::write(dir.join("config.toml"), text).unwrap();
    let out = sb.run(&sb.path("home"), &["sync", "--all"]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.contains("--vault"), "{}", out.stderr);
    let log = sb.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["stage"], "vault");
    assert_eq!(sb.calls("a"), None);
}

#[test]
fn hook_ignores_an_unset_vault() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.path("home/org")).unwrap();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    let out = sb.hook(&payload(&repo), &[], &[]);
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    assert!(sb.log().is_empty());
    assert_eq!(sb.calls("a"), None);
}

#[test]
fn hook_ignores_a_config_without_a_vault() {
    let sb = Sandbox::new();
    let repo = sb.repo("a", "a", &[("a-1", "first")]);
    let dir = sb.path("config/callisto");
    fs::create_dir_all(&dir).unwrap();
    let text = format!("repos = [{:?}]\n", repo.display().to_string());
    fs::write(dir.join("config.toml"), text).unwrap();
    let out = sb.hook(&payload(&repo), &[], &[]);
    assert_quiet_exit(&out);
    assert_eq!(out.stderr, "");
    assert!(sb.log().is_empty());
    assert_eq!(sb.calls("a"), None);
}

#[test]
fn help_names_no_default_vault() {
    let sb = Sandbox::new();
    let out = sb.run(&sb.path("home"), &["--help"]);
    assert_eq!(out.code, Some(0));
    assert!(!out.stdout.contains("~/org"), "{}", out.stdout);
}
