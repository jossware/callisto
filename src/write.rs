//! Writing a note atomically, so a killed run never leaves half a note.

use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The leading dot keeps the vault index from reading a leftover temp
/// file as a second marked note.
pub const TEMP_PREFIX: &str = ".callisto-";
pub const TEMP_SUFFIX: &str = ".tmp";
const NEW_NOTE_MODE: u32 = 0o644;

#[derive(Debug)]
pub enum Error {
    /// A new note's path already exists.
    Taken(PathBuf),
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Taken(path) => write!(f, "{} already exists", path.display()),
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Taken(_) => None,
        }
    }
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> Error + use<> {
    let path = path.to_path_buf();
    move |source| Error::Io { path, source }
}

/// Writes a new note. Never replaces an existing file: it may be a note
/// callisto did not write.
pub fn create(target: &Path, text: &str) -> Result<(), Error> {
    write(target, text, None)
}

/// Replaces a note, keeping its mode.
pub fn replace(target: &Path, text: &str) -> Result<(), Error> {
    let mode = fs::metadata(target)
        .map_err(io_at(target))?
        .permissions()
        .mode();
    write(target, text, Some(mode & 0o7777))
}

fn write(target: &Path, text: &str, mode: Option<u32>) -> Result<(), Error> {
    let dir = target.parent().unwrap_or(Path::new("."));
    sweep(dir);
    let mut tmp = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .suffix(TEMP_SUFFIX)
        .tempfile_in(dir)
        .map_err(io_at(dir))?;
    let tmp_path = tmp.path().to_path_buf();
    let file = tmp.as_file_mut();
    let perms = Permissions::from_mode(mode.unwrap_or(NEW_NOTE_MODE));
    file.write_all(text.as_bytes())
        .and_then(|()| file.set_permissions(perms))
        .and_then(|()| file.sync_all())
        .map_err(io_at(&tmp_path))?;
    let persisted = match mode {
        Some(_) => tmp.persist(target),
        None => tmp.persist_noclobber(target),
    };
    match persisted {
        Ok(_) => Ok(()),
        Err(e) if mode.is_none() && e.error.kind() == io::ErrorKind::AlreadyExists => {
            Err(Error::Taken(target.to_path_buf()))
        }
        Err(e) => Err(io_at(target)(e.error)),
    }
}

/// Names a temp file that a killed write may have left behind.
pub fn is_temp(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| n.starts_with(TEMP_PREFIX) && n.ends_with(TEMP_SUFFIX))
}

/// Only a killed run leaves a temp file behind: a live one holds the
/// sync lock.
fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if is_temp(&entry.file_name()) && entry.file_type().is_ok_and(|t| t.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn create_writes_0644() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("n.md");
        create(&target, "x").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "x");
        assert_eq!(mode_of(&target), 0o644);
    }

    #[test]
    fn create_refuses_existing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("n.md");
        fs::write(&target, "hand").unwrap();
        match create(&target, "x") {
            Err(Error::Taken(p)) => assert_eq!(p, target),
            other => panic!("expected Taken, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "hand");
    }

    #[test]
    fn replace_keeps_mode() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("n.md");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o600)).unwrap();
        replace(&target, "new").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(mode_of(&target), 0o600);
    }

    #[test]
    fn sweeps_stale_temp_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in [".callisto-abc.tmp", ".callisto-keep.md", "other.tmp"] {
            fs::write(root.join(name), "").unwrap();
        }
        fs::create_dir(root.join(".callisto-dir.tmp")).unwrap();
        std::os::unix::fs::symlink("other.tmp", root.join(".callisto-link.tmp")).unwrap();
        create(&root.join("n.md"), "x").unwrap();
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                ".callisto-dir.tmp",
                ".callisto-keep.md",
                ".callisto-link.tmp",
                "n.md",
                "other.tmp"
            ]
        );
    }

    #[test]
    fn missing_folder_is_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = create(&dir.path().join("absent/n.md"), "x").unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "{err:?}");
    }
}
