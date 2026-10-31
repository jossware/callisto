use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// Writes `text` to `path` as an executable script.
///
/// A child shell does the writing so this process never holds the file
/// open: a concurrent test's fork would inherit that descriptor, and
/// until the forked child execs, running the script fails with ETXTBSY.
pub fn write_script(path: &Path, text: &str) {
    let status = Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$1" > "$2""#, "sh", text])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "writing {}", path.display());
    fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
}
