use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Reduces a git remote URL to `host/path`, or `None` when it names no
/// host and path.
pub fn normalize(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if !["ssh", "git", "http", "https", "git+ssh", "ssh+git"].contains(&scheme.as_str()) {
                return None;
            }
            let (authority, path) = rest.split_once('/')?;
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            let host = host.split_once(':').map_or(host, |(h, _)| h);
            (host, path)
        }
        None => {
            let (host, path) = url.split_once(':')?;
            if host.contains('/') {
                return None;
            }
            let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
            // One letter is a Windows drive, not a host.
            if host.len() == 1 && host.as_bytes()[0].is_ascii_alphabetic() {
                return None;
            }
            (host, path)
        }
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{path}", host.to_ascii_lowercase()))
}

/// The git CLI, used only to read a checkout's origin remote.
#[derive(Debug, Clone)]
pub struct Git {
    program: OsString,
}

impl Default for Git {
    fn default() -> Git {
        Git::new("git")
    }
}

#[derive(Debug)]
pub enum RemoteError {
    Spawn(io::Error),
    Failed { stderr: String },
    NotUtf8,
    Unrecognised(String),
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteError::Spawn(source) => write!(f, "cannot run git: {source}"),
            RemoteError::Failed { stderr } => {
                write!(f, "{}", stderr.lines().next().unwrap_or("git failed"))
            }
            RemoteError::NotUtf8 => write!(f, "origin remote is not UTF-8"),
            RemoteError::Unrecognised(url) => {
                write!(f, "origin remote \"{url}\" is not a host/path URL")
            }
        }
    }
}

impl Error for RemoteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RemoteError::Spawn(source) => Some(source),
            _ => None,
        }
    }
}

impl Git {
    pub fn new(program: impl Into<OsString>) -> Git {
        Git {
            program: program.into(),
        }
    }

    fn command(&self, checkout: &Path) -> Command {
        let mut command = Command::new(&self.program);
        // Inherited, these would point git at another repo than `checkout`.
        command
            .arg("-C")
            .arg(checkout)
            .args(["remote", "get-url", "origin"])
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .stdin(Stdio::null());
        // `-C` still searches parents; a jj-only checkout inside another
        // git repo would otherwise report the outer repo's origin.
        if let Some(parent) = checkout.parent() {
            command.env("GIT_CEILING_DIRECTORIES", parent);
        }
        command
    }

    /// Reads `checkout`'s origin remote as `host/path`.
    pub fn origin(&self, checkout: &Path) -> Result<String, RemoteError> {
        let output = self
            .command(checkout)
            .output()
            .map_err(RemoteError::Spawn)?;
        if !output.status.success() {
            return Err(RemoteError::Failed {
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        let url = String::from_utf8(output.stdout).map_err(|_| RemoteError::NotUtf8)?;
        let url = url.trim();
        normalize(url).ok_or_else(|| RemoteError::Unrecognised(url.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bd::test_support::fake_bd;
    use std::path::Path;

    #[test]
    fn normalises_remote_forms() {
        for (url, want) in [
            ("git@github.com:o/p.git", "github.com/o/p"),
            ("github.com:o/p", "github.com/o/p"),
            ("https://github.com/o/p", "github.com/o/p"),
            ("https://user:tok@GitHub.com/o/p.git", "github.com/o/p"),
            ("ssh://git@GitHub.com:22/o/p.git/", "github.com/o/p"),
            ("git+ssh://git@host/a/b/c.git", "host/a/b/c"),
            ("ssh+git://host/a/b", "host/a/b"),
            ("git://host/a/b.git", "host/a/b"),
            ("http://host:8080/a/b", "host/a/b"),
            ("work-alias:o/p.git", "work-alias/o/p"),
            ("  https://h/o/p\n", "h/o/p"),
        ] {
            assert_eq!(normalize(url).as_deref(), Some(want), "{url}");
        }
    }

    #[test]
    fn rejects_non_remote_urls() {
        for url in [
            "",
            "/srv/git/p.git",
            "./p",
            "file:///srv/p.git",
            "https://h",
            "https:///o/p",
            "ftp://h/o/p",
            "host:",
            "c:\\p",
        ] {
            assert_eq!(normalize(url), None, "{url}");
        }
    }

    fn git(dir: &Path, body: &str) -> Git {
        Git::new(fake_bd(dir, body))
    }

    #[test]
    fn origin_reads_and_normalises() {
        let tmp = tempfile::tempdir().unwrap();
        let record = tmp.path().join("args");
        let body = format!(
            "echo \"$*\" > '{}'; echo git@GitHub.com:o/p.git",
            record.display()
        );
        let checkout = Path::new("/some/checkout");
        let got = git(tmp.path(), &body).origin(checkout).unwrap();
        assert_eq!(got, "github.com/o/p");
        let args = std::fs::read_to_string(record).unwrap();
        assert_eq!(args.trim(), "-C /some/checkout remote get-url origin");
    }

    #[test]
    fn origin_failure_carries_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let g = git(
            tmp.path(),
            "echo \"error: No such remote 'origin'\" >&2; exit 2",
        );
        match g.origin(tmp.path()) {
            Err(RemoteError::Failed { stderr }) => {
                assert_eq!(stderr, "error: No such remote 'origin'")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn origin_unrecognised_url() {
        let tmp = tempfile::tempdir().unwrap();
        let err = git(tmp.path(), "echo /srv/p.git")
            .origin(tmp.path())
            .unwrap_err();
        assert!(matches!(&err, RemoteError::Unrecognised(u) if u == "/srv/p.git"));
        assert!(err.to_string().contains("is not a host/path URL"), "{err}");
    }

    #[test]
    fn origin_clears_git_env() {
        let command = Git::default().command(Path::new("/srv/p"));
        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_owned())
            .collect();
        for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR"] {
            assert!(removed.contains(&key.into()), "{key} in {removed:?}");
        }
        let ceiling = command
            .get_envs()
            .find(|(key, _)| *key == "GIT_CEILING_DIRECTORIES")
            .and_then(|(_, value)| value);
        assert_eq!(ceiling, Some(Path::new("/srv").as_os_str()));
    }
}
