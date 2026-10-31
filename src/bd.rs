use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::Path;
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

    fn command(&self, checkout: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        // Either variable points bd at another repo's database, whose export
        // would read as this repo having lost every bead.
        command
            .args(args)
            .current_dir(checkout)
            .env_remove("BEADS_DIR")
            .env_remove("BEADS_DB")
            .stdin(Stdio::null());
        command
    }

    /// Runs bd in `checkout` and returns its stdout.
    pub fn run(&self, checkout: &Path, args: &[&str]) -> Result<String, BdError> {
        let output = self
            .command(checkout, args)
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

    /// Reads one `bd config` key in `checkout`. A blank, null or absent
    /// value counts as unset.
    pub fn config_get(&self, checkout: &Path, key: &str) -> Result<Option<String>, BdError> {
        // The text form prints `<key> (not set)` for an unset key, which a
        // value could also read as.
        let args = ["config", "get", "--json", key];
        let out = self.run(checkout, &args)?;
        let parsed: ConfigValue =
            serde_json::from_str(&out).map_err(|source| BdError::Malformed {
                args: args.join(" "),
                source,
            })?;
        let value = parsed.value.as_deref().map(str::trim).unwrap_or_default();
        Ok((!value.is_empty()).then(|| value.to_string()))
    }
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
    fn returns_stdout_and_runs_in_the_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r#"echo "$1 $(pwd -P)""#));
        let out = bd.run(tmp.path(), &["export"]).unwrap();
        let here = tmp.path().canonicalize().unwrap();
        assert_eq!(out.trim(), format!("export {}", here.display()));
    }

    #[test]
    fn nonzero_exit_is_an_error_with_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), "echo 'no database' >&2; exit 1"));
        let err = bd.run(tmp.path(), &["export"]).unwrap_err();
        assert!(matches!(err, BdError::Failed { .. }));
        assert!(err.to_string().contains("no database"), "{err}");
    }

    #[test]
    fn missing_bd_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Bd::new(tmp.path().join("absent"))
            .run(tmp.path(), &["export"])
            .unwrap_err();
        assert!(matches!(err, BdError::Spawn { .. }));
        assert!(err.to_string().contains("absent"), "{err}");
    }

    #[test]
    fn ignores_inherited_beads_location() {
        let bd = Bd::default();
        let command = bd.command(Path::new("."), &["export"]);
        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_owned())
            .collect();
        assert!(removed.contains(&"BEADS_DIR".into()), "{removed:?}");
        assert!(removed.contains(&"BEADS_DB".into()), "{removed:?}");
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
            bd.config_get(tmp.path(), "custom.obsidian.project")
                .unwrap()
                .as_deref(),
            Some("Example Project")
        );
    }

    #[test]
    fn unset_config_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), r#""""#);
        assert_eq!(bd.config_get(tmp.path(), "k").unwrap(), None);
    }

    #[test]
    fn null_config_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), "null");
        assert_eq!(bd.config_get(tmp.path(), "k").unwrap(), None);
    }

    #[test]
    fn missing_config_value_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r#"echo '{"key":"k"}'"#));
        assert_eq!(bd.config_get(tmp.path(), "k").unwrap(), None);
    }

    #[test]
    fn keyless_config_is_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r#"echo '{"error":"no"}'"#));
        let err = bd.config_get(tmp.path(), "k").unwrap_err();
        assert!(matches!(err, BdError::Malformed { .. }), "{err:?}");
    }

    #[test]
    fn blank_value_is_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), r#""  ""#);
        assert_eq!(bd.config_get(tmp.path(), "k").unwrap(), None);
    }

    #[test]
    fn not_set_lookalike_is_a_value() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = config_bd(tmp.path(), r#""x (not set)""#);
        assert_eq!(
            bd.config_get(tmp.path(), "k").unwrap().as_deref(),
            Some("x (not set)")
        );
    }

    #[test]
    fn non_json_config_is_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), "echo 'custom.x (not set)'"));
        let err = bd.config_get(tmp.path(), "custom.x").unwrap_err();
        assert!(matches!(err, BdError::Malformed { .. }), "{err:?}");
        assert!(err.to_string().contains("config get"), "{err}");
    }

    #[test]
    fn non_utf8_output_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(tmp.path(), r"printf '\377'"));
        assert!(matches!(
            bd.run(tmp.path(), &["export"]),
            Err(BdError::NotUtf8 { .. })
        ));
    }
}
