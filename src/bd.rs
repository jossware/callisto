use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use serde::Deserialize;

/// The bd CLI, callisto's only way into beads.
#[derive(Debug, Clone)]
pub struct Bd {
    program: OsString,
}

impl Default for Bd {
    fn default() -> Bd {
        Bd::new("bd")
    }
}

#[derive(Debug)]
pub enum BdError {
    Spawn {
        program: OsString,
        source: io::Error,
    },
    Failed {
        args: String,
        status: ExitStatus,
        stderr: String,
    },
    NotUtf8 {
        args: String,
    },
    Malformed {
        args: String,
        source: serde_json::Error,
    },
}

impl fmt::Display for BdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BdError::Spawn { program, source } => {
                write!(f, "cannot run {}: {source}", program.to_string_lossy())
            }
            BdError::Failed {
                args,
                status,
                stderr,
            } => write!(f, "bd {args} failed ({status}): {}", stderr.trim()),
            BdError::NotUtf8 { args } => {
                write!(f, "bd {args} printed output that is not UTF-8")
            }
            BdError::Malformed { args, source } => {
                write!(f, "bd {args} printed unexpected output: {source}")
            }
        }
    }
}

impl Error for BdError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            BdError::Spawn { source, .. } => Some(source),
            BdError::Malformed { source, .. } => Some(source),
            BdError::Failed { .. } | BdError::NotUtf8 { .. } => None,
        }
    }
}

impl Bd {
    pub fn new(program: impl Into<OsString>) -> Bd {
        Bd {
            program: program.into(),
        }
    }

    fn command(&self, dir: &Path, args: &[&str], target: Target<'_>) -> Command {
        let mut command = Command::new(&self.program);
        // BEADS_DB points bd at another repo's database, whose export would
        // read as this repo having lost every bead.
        command
            .args(args)
            .current_dir(dir)
            .env_remove("BEADS_DB")
            .stdin(Stdio::null());
        if let Target::Database(database) = target {
            command.env("BEADS_DIR", database);
        }
        command
    }

    /// Runs bd against the beads database at `database`.
    pub fn run_on(&self, database: &Path, args: &[&str]) -> Result<String, BdError> {
        self.exec(database, args, Target::Database(database))
    }

    fn exec(&self, dir: &Path, args: &[&str], target: Target<'_>) -> Result<String, BdError> {
        let output = self
            .command(dir, args, target)
            .output()
            .map_err(|source| BdError::Spawn {
                program: self.program.clone(),
                source,
            })?;
        let args = args.join(" ");
        if !output.status.success() {
            return Err(BdError::Failed {
                args,
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        String::from_utf8(output.stdout).map_err(|_| BdError::NotUtf8 { args })
    }

    /// Reads one `bd config` key of the database at `database`. A blank,
    /// null or absent value counts as unset.
    pub fn config_get_on(&self, database: &Path, key: &str) -> Result<Option<String>, BdError> {
        // The text form prints `<key> (not set)` for an unset key, which a
        // value could also read as.
        let args = ["config", "get", "--json", key];
        parse_config(&args, &self.run_on(database, &args)?)
    }

    /// Asks bd, run in `dir`, which database it would use there. The
    /// caller's `BEADS_DIR` still applies.
    pub fn lookup(&self, dir: &Path) -> Result<Context, BdError> {
        let args = ["context", "--json"];
        let out = self.exec(dir, &args, Target::Inherit)?;
        parse_context(&args, &out)
    }
}

/// Which beads database a bd call reaches.
#[derive(Clone, Copy)]
enum Target<'a> {
    Inherit,
    Database(&'a Path),
}

fn parse_config(args: &[&str], out: &str) -> Result<Option<String>, BdError> {
    let parsed: ConfigValue = serde_json::from_str(out).map_err(|source| BdError::Malformed {
        args: args.join(" "),
        source,
    })?;
    let value = parsed.value.as_deref().map(str::trim).unwrap_or_default();
    Ok((!value.is_empty()).then(|| value.to_string()))
}

fn parse_context(args: &[&str], out: &str) -> Result<Context, BdError> {
    serde_json::from_str(out).map_err(|source| BdError::Malformed {
        args: args.join(" "),
        source,
    })
}

/// What `bd context` reports about a checkout.
#[derive(Debug, Deserialize)]
pub struct Context {
    pub beads_dir: PathBuf,
}

#[derive(Deserialize)]
struct ConfigValue {
    /// Required, so that a reply without it reads as malformed rather
    /// than as an unset value.
    #[serde(rename = "key")]
    _key: String,
    value: Option<String>,
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// Writes an executable `bd` stand-in running `body` under /bin/sh.
    ///
    /// A child shell does the writing so this process never holds the
    /// file open: a concurrent test's fork would inherit that descriptor,
    /// and until the forked child execs, running the stand-in fails with
    /// ETXTBSY.
    pub fn fake_bd(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("bd");
        let status = Command::new("/bin/sh")
            .args(["-c", r#"printf '%s' "$1" > "$2""#, "sh"])
            .arg(format!("#!/bin/sh\n{body}\n"))
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "writing {}", path.display());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::fake_bd;
    use super::*;

    #[test]
    fn returns_stdout() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r#"echo "$1 $(pwd -P)""#));
        let out = bd.run_on(tmp.path(), &["export"]).unwrap();
        let here = tmp.path().canonicalize().unwrap();
        assert_eq!(out.trim(), format!("export {}", here.display()));
    }

    #[test]
    fn nonzero_exit_is_an_error_with_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), "echo 'no database' >&2; exit 1"));
        let err = bd.run_on(tmp.path(), &["export"]).unwrap_err();
        assert!(matches!(err, BdError::Failed { .. }));
        assert!(err.to_string().contains("no database"), "{err}");
    }

    #[test]
    fn missing_bd_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Bd::new(tmp.path().join("absent"))
            .run_on(tmp.path(), &["export"])
            .unwrap_err();
        assert!(matches!(err, BdError::Spawn { .. }));
        assert!(err.to_string().contains("absent"), "{err}");
    }

    #[test]
    fn database_calls_set_beads_dir_and_drop_beads_db() {
        let command = Bd::default().command(
            Path::new("/d"),
            &["export"],
            Target::Database(Path::new("/d")),
        );
        let envs: Vec<_> = command.get_envs().collect();
        assert!(
            envs.contains(&("BEADS_DIR".as_ref(), Some("/d".as_ref()))),
            "{envs:?}"
        );
        assert!(envs.contains(&("BEADS_DB".as_ref(), None)), "{envs:?}");
    }

    #[test]
    fn lookup_keeps_beads_dir_and_drops_beads_db() {
        let command = Bd::default().command(Path::new("."), &["context"], Target::Inherit);
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&("BEADS_DB".as_ref(), None)), "{envs:?}");
        assert!(!envs.iter().any(|(key, _)| *key == "BEADS_DIR"), "{envs:?}");
    }

    #[test]
    fn run_on_sets_beads_dir_and_runs_in_the_database() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(
            tools.path(),
            r#"echo "$BEADS_DIR|${BEADS_DB-unset}|$(pwd -P)""#,
        ));
        let out = bd.run_on(tmp.path(), &["export"]).unwrap();
        let here = tmp.path().canonicalize().unwrap();
        assert_eq!(
            out.trim(),
            format!("{}|unset|{}", tmp.path().display(), here.display())
        );
    }

    #[test]
    fn config_get_on_reads_the_named_database() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("prefix"), "p").unwrap();
        let bd = Bd::new(fake_bd(
            tools.path(),
            r#"printf '{"key":"issue_prefix","value":"%s"}\n' "$(cat "$BEADS_DIR/prefix")""#,
        ));
        assert_eq!(
            bd.config_get_on(tmp.path(), "issue_prefix")
                .unwrap()
                .as_deref(),
            Some("p")
        );
    }

    #[test]
    fn lookup_runs_in_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(
            tools.path(),
            r#"printf '{"beads_dir":"%s/.beads"}\n' "$(pwd -P)""#,
        ));
        let context = bd.lookup(tmp.path()).unwrap();
        let here = tmp.path().canonicalize().unwrap();
        assert_eq!(context.beads_dir, here.join(".beads"));
    }

    fn config_bd(dir: &Path, value: &str) -> Bd {
        Bd::new(fake_bd(
            dir,
            &format!(
                r#"[ "$1 $2 $3" = "config get --json" ] || exit 9
printf '{{"key":"%s","schema_version":1,"value":%s}}\n' "$4" '{value}'"#
            ),
        ))
    }

    #[test]
    fn config_value_is_trimmed() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), r#"" Example Project ""#);
        assert_eq!(
            bd.config_get_on(tmp.path(), "custom.obsidian.project")
                .unwrap()
                .as_deref(),
            Some("Example Project")
        );
    }

    #[test]
    fn unset_config_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), r#""""#);
        assert_eq!(bd.config_get_on(tmp.path(), "k").unwrap(), None);
    }

    #[test]
    fn null_config_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), "null");
        assert_eq!(bd.config_get_on(tmp.path(), "k").unwrap(), None);
    }

    #[test]
    fn missing_config_value_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r#"echo '{"key":"k"}'"#));
        assert_eq!(bd.config_get_on(tmp.path(), "k").unwrap(), None);
    }

    #[test]
    fn keyless_config_is_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r#"echo '{"error":"no"}'"#));
        let err = bd.config_get_on(tmp.path(), "k").unwrap_err();
        assert!(matches!(err, BdError::Malformed { .. }), "{err:?}");
    }

    #[test]
    fn blank_value_is_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), r#""  ""#);
        assert_eq!(bd.config_get_on(tmp.path(), "k").unwrap(), None);
    }

    #[test]
    fn not_set_lookalike_is_a_value() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), r#""x (not set)""#);
        assert_eq!(
            bd.config_get_on(tmp.path(), "k").unwrap().as_deref(),
            Some("x (not set)")
        );
    }

    #[test]
    fn non_json_config_is_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), "echo 'custom.x (not set)'"));
        let err = bd.config_get_on(tmp.path(), "custom.x").unwrap_err();
        assert!(matches!(err, BdError::Malformed { .. }), "{err:?}");
        assert!(err.to_string().contains("config get"), "{err}");
    }

    #[test]
    fn non_utf8_output_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r"printf '\377'"));
        assert!(matches!(
            bd.run_on(tmp.path(), &["export"]),
            Err(BdError::NotUtf8 { .. })
        ));
    }

    #[test]
    fn lookup_reads_beads_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(
            tmp.path(),
            r#"[ "$1 $2" = "context --json" ] || exit 9
echo '{"backend":"dolt","beads_dir":"/x/.beads","is_redirected":true}'"#,
        ));
        let context = bd.lookup(tmp.path()).unwrap();
        assert_eq!(context.beads_dir, PathBuf::from("/x/.beads"));
    }

    #[test]
    fn lookup_without_beads_dir_is_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r#"echo '{"backend":"dolt"}'"#));
        let err = bd.lookup(tmp.path()).unwrap_err();
        assert!(matches!(err, BdError::Malformed { .. }), "{err:?}");
        assert!(err.to_string().contains("context --json"), "{err}");
    }

    #[test]
    fn lookup_failure_keeps_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(
            tmp.path(),
            "echo 'no .beads directory found' >&2; exit 1",
        ));
        let err = bd.lookup(tmp.path()).unwrap_err();
        assert!(matches!(err, BdError::Failed { .. }), "{err:?}");
        assert!(
            err.to_string().contains("no .beads directory found"),
            "{err}"
        );
    }
}
