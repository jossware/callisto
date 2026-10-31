//! The repo a path belongs to, resolved to its main checkout.
//!
//! bd runs in the main checkout: a non-colocated jj workspace has no
//! `.git`, so bd cannot find the database from there.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::bd::{Bd, BdError};

pub const PREFIX_KEY: &str = "issue_prefix";
pub const PROJECT_KEY: &str = "custom.obsidian.project";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The main checkout, canonical.
    pub root: PathBuf,
    /// The main checkout's directory name. Leading dots are kept, so
    /// `~/.dotfiles` needs a project override.
    pub name: String,
}

impl Repo {
    pub fn has_beads(&self) -> bool {
        self.root.join(".beads").is_dir()
    }
}

/// What a repo's bd config says about syncing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub prefix: String,
    /// The vault project to file under, when it is not the repo's name.
    pub project: Option<String>,
}

#[derive(Debug)]
pub enum SettingsError {
    Bd(BdError),
    NoPrefix,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Bd(e) => e.fmt(f),
            SettingsError::NoPrefix => write!(f, "bd config has no {PREFIX_KEY}"),
        }
    }
}

impl Error for SettingsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            SettingsError::Bd(e) => Some(e),
            SettingsError::NoPrefix => None,
        }
    }
}

impl From<BdError> for SettingsError {
    fn from(e: BdError) -> SettingsError {
        SettingsError::Bd(e)
    }
}

/// A repo without a prefix is an error: no note could be scoped to it.
pub fn settings(bd: &Bd, repo: &Repo) -> Result<Settings, SettingsError> {
    let prefix = bd
        .config_get(&repo.root, PREFIX_KEY)?
        .ok_or(SettingsError::NoPrefix)?;
    let project = bd.config_get(&repo.root, PROJECT_KEY)?;
    Ok(Settings { prefix, project })
}

enum Level {
    NoMarker,
    Broken,
    Checkout(Repo),
}

/// `None` when `dir` is in no checkout, or the nearest checkout's pointer
/// is broken. `dir` need not exist: a hook's cwd may have been deleted.
pub fn locate(dir: &Path) -> Option<Repo> {
    let dir = std::path::absolute(dir).ok()?;
    for level in dir.ancestors() {
        match markers(level) {
            Level::NoMarker => {}
            Level::Broken => return None,
            Level::Checkout(repo) => return Some(repo),
        }
    }
    None
}

fn markers(level: &Path) -> Level {
    let jj = level.join(".jj").join("repo");
    match fs::metadata(&jj) {
        Ok(meta) if meta.is_dir() => return checkout(level, None),
        Ok(_) => return jj_workspace(level, &jj),
        Err(_) if present(&jj) => return Level::Broken,
        Err(_) => {}
    }
    let git = level.join(".git");
    match fs::metadata(&git) {
        Ok(meta) if meta.is_dir() => checkout(level, None),
        Ok(_) => git_file(level, &git),
        Err(_) if present(&git) => Level::Broken,
        Err(_) => Level::NoMarker,
    }
}

/// The marker entry exists, whether or not its target can be reached. One
/// that cannot be followed is a broken pointer.
fn present(marker: &Path) -> bool {
    fs::symlink_metadata(marker).is_ok()
}

/// `name` overrides the root's own name, for worktrees of a bare repo.
fn checkout(root: &Path, name: Option<String>) -> Level {
    match fs::canonicalize(root) {
        Ok(root) => Level::Checkout(Repo {
            name: name.unwrap_or_else(|| basename(&root)),
            root,
        }),
        Err(_) => Level::Broken,
    }
}

fn git_file(level: &Path, file: &Path) -> Level {
    let contents = read_pointer(file).unwrap_or_default();
    let gitdir = contents
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .and_then(|target| follow(level, target));
    let Some(gitdir) = gitdir else {
        return Level::Broken;
    };
    let commondir = gitdir.join("commondir");
    if !present(&commondir) {
        // Submodules and `--separate-git-dir` checkouts own their git dir outright.
        return checkout(level, None);
    }
    let Some(common) = read_pointer(&commondir).and_then(|target| follow(&gitdir, &target)) else {
        return Level::Broken;
    };
    match common.file_name() {
        Some(own) if own == ".git" => match common.parent() {
            Some(main) => checkout(main, None),
            None => Level::Broken,
        },
        // A bare repo has no main checkout; this worktree stands in for it.
        _ => checkout(level, Some(bare_name(&common))),
    }
}

/// A FIFO would block the read, and with it the hook.
fn read_pointer(file: &Path) -> Option<String> {
    fs::metadata(file).ok().filter(fs::Metadata::is_file)?;
    fs::read_to_string(file).ok()
}

/// Every pointer names a git or jj directory. A target that is not one is a
/// broken pointer.
fn follow(base: &Path, target: &str) -> Option<PathBuf> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    fs::canonicalize(base.join(target))
        .ok()
        .filter(|path| path.is_dir())
}

fn bare_name(common: &Path) -> String {
    let own = basename(common);
    // `foo/.bare` is a convention for keeping worktrees beside a bare repo.
    if own == ".bare" {
        return common.parent().map(basename).unwrap_or_default();
    }
    own.strip_suffix(".git").unwrap_or(&own).to_string()
}

fn jj_workspace(level: &Path, pointer: &Path) -> Level {
    let repo = read_pointer(pointer).and_then(|target| follow(&level.join(".jj"), &target));
    match repo
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
    {
        Some(main) => checkout(main, None),
        None => Level::Broken,
    }
}

fn basename(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bd::BdError;
    use crate::bd::test_support::fake_bd;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn mkdir(p: &Path) {
        fs::create_dir_all(p).unwrap();
    }

    fn write(p: &Path, contents: &str) {
        mkdir(p.parent().unwrap());
        fs::write(p, contents).unwrap();
    }

    fn repo(root: &Path, name: &str) -> Option<Repo> {
        Some(Repo {
            root: root.canonicalize().unwrap(),
            name: name.into(),
        })
    }

    /// Lays out a linked worktree the way git 2.55 does.
    fn link_worktree(common: &Path, wt: &Path) {
        let admin = common.join("worktrees").join(wt.file_name().unwrap());
        write(&admin.join("commondir"), "../..\n");
        write(&wt.join(".git"), &format!("gitdir: {}\n", admin.display()));
    }

    #[test]
    fn git_directory_is_the_checkout() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".git"));
        mkdir(&proj.join("src/deep"));
        assert_eq!(locate(&proj), repo(&proj, "proj"));
        assert_eq!(locate(&proj.join("src/deep")), repo(&proj, "proj"));
    }

    #[test]
    fn jj_repo_directory_is_the_checkout() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".jj/repo"));
        mkdir(&proj.join("src"));
        assert_eq!(locate(&proj.join("src")), repo(&proj, "proj"));
    }

    #[test]
    fn colocated_jj_and_git_is_the_checkout() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".jj/repo"));
        mkdir(&proj.join(".git"));
        assert_eq!(locate(&proj), repo(&proj, "proj"));
    }

    #[test]
    fn jj_without_repo_is_not_a_marker() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".git"));
        mkdir(&proj.join("sub/.jj"));
        assert_eq!(locate(&proj.join("sub")), repo(&proj, "proj"));
    }

    #[test]
    fn missing_dir_inside_a_checkout_resolves_to_it() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".git"));
        assert_eq!(
            locate(&proj.join(".workspaces/gone/src")),
            repo(&proj, "proj")
        );
    }

    #[test]
    fn no_marker_is_not_a_repo() {
        let root = tmp();
        let dir = root.path().join("a/b");
        mkdir(&dir);
        assert_eq!(locate(&dir), None);
    }

    #[test]
    fn beads_outside_a_checkout_is_not_a_repo() {
        let root = tmp();
        let dir = root.path().join("d");
        mkdir(&dir.join(".beads"));
        assert_eq!(locate(&dir), None);
    }

    #[test]
    fn leading_dots_are_kept() {
        let root = tmp();
        let dotfiles = root.path().join(".dotfiles");
        mkdir(&dotfiles.join(".git"));
        assert_eq!(locate(&dotfiles), repo(&dotfiles, ".dotfiles"));
    }

    #[test]
    fn worktree_resolves_to_main() {
        let root = tmp();
        let main = root.path().join("main");
        let wt = root.path().join("wt");
        mkdir(&main.join(".git"));
        link_worktree(&main.join(".git"), &wt);
        mkdir(&wt.join("src/deep"));
        assert_eq!(locate(&wt), repo(&main, "main"));
        assert_eq!(locate(&wt.join("src/deep")), repo(&main, "main"));
    }

    #[test]
    fn worktree_with_relative_gitdir() {
        let root = tmp();
        let main = root.path().join("main");
        let wt = root.path().join("wt");
        mkdir(&main.join(".git"));
        link_worktree(&main.join(".git"), &wt);
        write(&wt.join(".git"), "gitdir: ../main/.git/worktrees/wt\n");
        assert_eq!(locate(&wt), repo(&main, "main"));
    }

    #[test]
    fn pointer_whitespace_is_trimmed() {
        let root = tmp();
        let main = root.path().join("main");
        let wt = root.path().join("wt");
        mkdir(&main.join(".git"));
        link_worktree(&main.join(".git"), &wt);
        let admin = main.join(".git/worktrees/wt");
        write(
            &wt.join(".git"),
            &format!("gitdir: {}  \r\n", admin.display()),
        );
        write(&admin.join("commondir"), "../..\r\n");
        assert_eq!(locate(&wt), repo(&main, "main"));
    }

    #[test]
    fn bare_common_dir_uses_the_worktree() {
        let root = tmp();
        let bare = root.path().join("krew-index");
        mkdir(&bare.join("objects"));
        link_worktree(&bare, &bare.join("main"));
        assert_eq!(
            locate(&bare.join("main")),
            repo(&bare.join("main"), "krew-index")
        );
    }

    #[test]
    fn bare_common_dir_drops_git_suffix() {
        let root = tmp();
        let bare = root.path().join("foo.git");
        let wt = root.path().join("foo-wt");
        mkdir(&bare);
        link_worktree(&bare, &wt);
        assert_eq!(locate(&wt), repo(&wt, "foo"));
    }

    #[test]
    fn hidden_bare_common_dir_defers_to_its_parent() {
        let root = tmp();
        let foo = root.path().join("foo");
        mkdir(&foo.join(".bare"));
        link_worktree(&foo.join(".bare"), &foo.join("main"));
        assert_eq!(locate(&foo.join("main")), repo(&foo.join("main"), "foo"));
    }

    #[test]
    fn submodule_is_its_own_checkout() {
        let root = tmp();
        let sup = root.path().join("sup");
        mkdir(&sup.join(".git/modules/themes/sub"));
        write(
            &sup.join("themes/sub/.git"),
            "gitdir: ../../.git/modules/themes/sub\n",
        );
        let sub = sup.join("themes/sub");
        mkdir(&sub.join("layouts"));
        assert_eq!(locate(&sub.join("layouts")), repo(&sub, "sub"));
    }

    #[test]
    fn jj_workspace_resolves_to_main() {
        let root = tmp();
        let main = root.path().join("main");
        let ws = root.path().join("ws1");
        mkdir(&main.join(".jj/repo"));
        // jj 0.44 writes the pointer relative to the workspace's `.jj`.
        write(&ws.join(".jj/repo"), "../../main/.jj/repo");
        mkdir(&ws.join("src"));
        assert_eq!(locate(&ws), repo(&main, "main"));
        assert_eq!(locate(&ws.join("src")), repo(&main, "main"));
    }

    #[test]
    fn jj_workspace_nested_in_main() {
        let root = tmp();
        let main = root.path().join("main");
        let ws = main.join(".workspaces/ws");
        mkdir(&main.join(".jj/repo"));
        write(&ws.join(".jj/repo"), "../../../.jj/repo");
        assert_eq!(locate(&ws), repo(&main, "main"));
    }

    #[test]
    fn jj_workspace_with_absolute_pointer() {
        let root = tmp();
        let main = root.path().join("main");
        let ws = root.path().join("ws1");
        mkdir(&main.join(".jj/repo"));
        write(
            &ws.join(".jj/repo"),
            &main.join(".jj/repo").display().to_string(),
        );
        assert_eq!(locate(&ws), repo(&main, "main"));
    }

    #[test]
    fn broken_jj_pointer_is_not_a_repo() {
        let root = tmp();
        let outer = root.path().join("outer");
        mkdir(&outer.join(".git"));
        let ws = outer.join("ws1");
        write(&ws.join(".jj/repo"), "../../gone/.jj/repo");
        assert_eq!(locate(&ws.join("src")), None);
    }

    #[test]
    fn broken_git_pointers_are_not_repos() {
        let root = tmp();
        let outer = root.path().join("outer");
        mkdir(&outer.join(".git"));
        let main = outer.join("main");
        mkdir(&main.join(".git"));

        let missing_gitdir = outer.join("wt1");
        write(
            &missing_gitdir.join(".git"),
            &format!("gitdir: {}\n", root.path().join("nowhere").display()),
        );

        let missing_common = outer.join("wt2");
        link_worktree(&main.join(".git"), &missing_common);
        write(
            &main.join(".git/worktrees/wt2/commondir"),
            "../../missing\n",
        );

        let empty = outer.join("wt3");
        write(&empty.join(".git"), "");

        let no_prefix = outer.join("wt4");
        write(&no_prefix.join(".git"), "nonsense\n");

        let empty_target = outer.join("wt5");
        write(&empty_target.join(".git"), "gitdir: \n");

        for wt in [
            missing_gitdir,
            missing_common,
            empty,
            no_prefix,
            empty_target,
        ] {
            assert_eq!(locate(&wt.join("src")), None, "{}", wt.display());
        }
    }

    #[test]
    fn pointers_to_files_are_not_repos() {
        let root = tmp();
        let main = root.path().join("main");
        mkdir(&main.join(".git"));
        let stray = root.path().join("stray");
        write(&stray, "not a repo\n");

        let git_to_file = root.path().join("wt1");
        write(
            &git_to_file.join(".git"),
            &format!("gitdir: {}\n", stray.display()),
        );

        let common_to_file = root.path().join("wt2");
        link_worktree(&main.join(".git"), &common_to_file);
        write(
            &main.join(".git/worktrees/wt2/commondir"),
            &stray.display().to_string(),
        );

        let jj_to_file = root.path().join("ws1");
        write(&jj_to_file.join(".jj/repo"), &stray.display().to_string());

        for dir in [git_to_file, common_to_file, jj_to_file] {
            assert_eq!(locate(&dir), None, "{}", dir.display());
        }
    }

    #[test]
    fn dangling_marker_symlinks_are_not_repos() {
        use std::os::unix::fs::symlink;

        let root = tmp();
        let outer = root.path().join("outer");
        mkdir(&outer.join(".git"));
        let git = outer.join("wt1");
        mkdir(&git);
        symlink(root.path().join("gone"), git.join(".git")).unwrap();
        let jj = outer.join("ws1");
        mkdir(&jj.join(".jj"));
        symlink(root.path().join("gone"), jj.join(".jj/repo")).unwrap();
        let main = root.path().join("main");
        mkdir(&main.join(".git"));
        let common = outer.join("wt2");
        link_worktree(&main.join(".git"), &common);
        let commondir = main.join(".git/worktrees/wt2/commondir");
        fs::remove_file(&commondir).unwrap();
        symlink(root.path().join("gone"), &commondir).unwrap();

        for dir in [git, jj, common] {
            assert_eq!(locate(&dir.join("src")), None, "{}", dir.display());
        }
    }

    #[test]
    fn non_regular_pointers_are_not_repos() {
        use std::process::Command;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let root = tmp();
        let main = root.path().join("main");
        mkdir(&main.join(".git"));
        let fifo_git = root.path().join("wt1");
        let fifo_common = root.path().join("wt2");
        let fifo_jj = root.path().join("ws1");
        mkdir(&fifo_git);
        link_worktree(&main.join(".git"), &fifo_common);
        let commondir = main.join(".git/worktrees/wt2/commondir");
        fs::remove_file(&commondir).unwrap();
        mkdir(&fifo_jj.join(".jj"));
        for fifo in [fifo_git.join(".git"), commondir, fifo_jj.join(".jj/repo")] {
            assert!(
                Command::new("mkfifo")
                    .arg(&fifo)
                    .status()
                    .unwrap()
                    .success()
            );
        }

        for dir in [fifo_git, fifo_common, fifo_jj] {
            let (tx, rx) = mpsc::channel();
            let probe: PathBuf = dir.clone();
            thread::spawn(move || tx.send(locate(&probe)).unwrap());
            let got = rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| panic!("{}: blocked reading a FIFO", dir.display()));
            assert_eq!(got, None, "{}", dir.display());
        }
    }

    /// Answers `config get --json <key>` from `pairs` and records each run's
    /// directory in `<dir>/pwd`.
    fn config_bd(dir: &Path, pairs: &[(&str, &str)]) -> Bd {
        let mut cases = String::new();
        for (key, value) in pairs {
            cases.push_str(&format!("  {key}) v='{value}' ;;\n"));
        }
        Bd::new(fake_bd(
            dir,
            &format!(
                r#"pwd -P >> "{log}"
case "$4" in
{cases}  *) v='' ;;
esac
printf '{{"key":"%s","schema_version":1,"value":"%s"}}\n' "$4" "$v""#,
                log = dir.join("pwd").display()
            ),
        ))
    }

    #[test]
    fn settings_run_in_the_main_checkout() {
        let root = tmp();
        let main = root.path().join("main");
        let wt = root.path().join("wt");
        mkdir(&main.join(".git"));
        link_worktree(&main.join(".git"), &wt);
        let bd = config_bd(
            root.path(),
            &[(PREFIX_KEY, "zz"), (PROJECT_KEY, "Example Project")],
        );
        let found = locate(&wt).unwrap();
        assert_eq!(
            settings(&bd, &found).unwrap(),
            Settings {
                prefix: "zz".into(),
                project: Some("Example Project".into()),
            }
        );
        let pwds = fs::read_to_string(root.path().join("pwd")).unwrap();
        let main = main.canonicalize().unwrap();
        assert_eq!(pwds.lines().count(), 2);
        for pwd in pwds.lines() {
            assert_eq!(Path::new(pwd), main);
        }
    }

    #[test]
    fn settings_without_override() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".git"));
        let bd = config_bd(root.path(), &[(PREFIX_KEY, "zz")]);
        let got = settings(&bd, &locate(&proj).unwrap()).unwrap();
        assert_eq!(got.project, None);
    }

    #[test]
    fn missing_prefix_is_an_error() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".git"));
        let bd = config_bd(root.path(), &[(PROJECT_KEY, "x")]);
        let err = settings(&bd, &locate(&proj).unwrap()).unwrap_err();
        assert!(matches!(err, SettingsError::NoPrefix), "{err:?}");
        assert!(err.to_string().contains("issue_prefix"), "{err}");
    }

    #[test]
    fn bd_failure_is_an_error() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".git"));
        let bd = Bd::new(fake_bd(root.path(), "echo 'no database' >&2; exit 1"));
        let err = settings(&bd, &locate(&proj).unwrap()).unwrap_err();
        assert!(
            matches!(err, SettingsError::Bd(BdError::Failed { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn has_beads() {
        let root = tmp();
        let proj = root.path().join("proj");
        mkdir(&proj.join(".git"));
        let found = locate(&proj).unwrap();
        assert!(!found.has_beads());
        write(&proj.join(".beads"), "");
        assert!(!found.has_beads());
        fs::remove_file(proj.join(".beads")).unwrap();
        mkdir(&proj.join(".beads"));
        assert!(found.has_beads());
    }
}
