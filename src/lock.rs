//! Advisory file locks.

use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_secs(10);
/// The wait a test gives a lock it expects to be free. A child process
/// spawned by another test briefly holds copies of this process's
/// descriptors, so a free lock can read as held for an instant.
#[cfg(test)]
pub const FREE: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_millis(50);

/// Held until dropped. The OS releases it when the process exits, so a
/// crash cannot leave it stale.
#[derive(Debug)]
#[must_use = "dropping it releases the lock"]
pub struct Lock {
    _file: File,
}

#[derive(Debug)]
pub enum Error {
    /// Another run holds the lock.
    Locked,
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Locked => f.write_str("locked by another run"),
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Locked => None,
        }
    }
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> Error + use<> {
    let path = path.to_path_buf();
    move |source| Error::Io { path, source }
}

fn open(path: &Path) -> Result<File, Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_at(parent))?;
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(io_at(path))
}

/// Waits as long as it takes for an exclusive lock on `path`, creating
/// the file and its folder if missing. A sync queued behind another
/// run's pull is still correct; giving up would only lose it.
pub fn acquire(path: &Path) -> Result<Lock, Error> {
    let file = open(path)?;
    file.lock().map_err(io_at(path))?;
    Ok(Lock { _file: file })
}

/// Takes the lock only if it is free right now.
pub fn try_acquire(path: &Path) -> Result<Lock, Error> {
    acquire_within(path, Duration::ZERO)
}

/// Waits up to `timeout` for the lock.
pub fn acquire_within(path: &Path, timeout: Duration) -> Result<Lock, Error> {
    let file = open(path)?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Lock { _file: file }),
            Err(TryLockError::WouldBlock) => match pause(Instant::now(), deadline) {
                Some(wait) => thread::sleep(wait),
                None => return Err(Error::Locked),
            },
            Err(TryLockError::Error(e)) => return Err(io_at(path)(e)),
        }
    }
}

/// How long to wait before the next attempt, or `None` once the deadline
/// has passed. The last wait ends at the deadline for a final attempt.
fn pause(now: Instant, deadline: Instant) -> Option<Duration> {
    let left = deadline.checked_duration_since(now)?;
    (!left.is_zero()).then(|| RETRY.min(left))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn second_acquirer_sees_the_lock_held() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sync.lock");
        let first = acquire(&path).unwrap();
        assert!(matches!(try_acquire(&path), Err(Error::Locked)));
        assert!(matches!(
            acquire_within(&path, Duration::ZERO),
            Err(Error::Locked)
        ));
        drop(first);
        let _lock = acquire_within(&path, FREE).unwrap();
    }

    #[test]
    fn acquire_waits_for_the_holder() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sync.lock");
        let held = acquire(&path).unwrap();
        let start = Instant::now();
        let holder = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        let _lock = acquire(&path).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(150));
        holder.join().unwrap();
    }

    #[test]
    fn creates_the_missing_state_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/sync.lock");
        let _lock = acquire(&path).unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn pause_stops_at_the_deadline() {
        let now = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(pause(now, now + ms(120)), Some(RETRY));
        assert_eq!(pause(now, now + ms(20)), Some(ms(20)));
        assert_eq!(pause(now, now), None);
        assert_eq!(pause(now + ms(1), now), None);
    }

    #[test]
    fn unusable_path_names_the_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("state");
        fs::write(&blocker, "a file, not a folder").unwrap();
        let path = blocker.join("x.lock");
        for err in [acquire(&path).unwrap_err(), try_acquire(&path).unwrap_err()] {
            assert!(
                matches!(&err, Error::Io { path, .. } if *path == blocker),
                "{err}"
            );
        }
    }
}
