//! The vault project or area a repo's notes are filed under.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::repo::{PROJECT_KEY, Repo, Settings};

/// Where a project or area's folder may live, in lookup order.
pub const LOCATIONS: [&str; 4] = ["Projects", "Areas", "Archives/Projects", "Archives/Areas"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    /// The home note, relative to the vault.
    pub home: PathBuf,
}

impl Project {
    /// Where new notes go.
    pub fn folder(&self) -> &Path {
        self.home.parent().unwrap_or(Path::new(""))
    }
}

/// A repo with no project to file under. It is skipped, never given one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmapped {
    pub name: String,
    pub overridden: bool,
}

impl fmt::Display for Unmapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = if self.overridden {
            format!(" (set by {PROJECT_KEY})")
        } else {
            String::new()
        };
        write!(
            f,
            "no project or area named \"{}\" in the vault{source}; skipping",
            self.name
        )
    }
}

/// A set override is final: falling back to the repo name would hide a
/// typo in it.
pub fn map(vault: &Path, repo: &Repo, settings: &Settings) -> Result<Project, Unmapped> {
    let (name, overridden) = match &settings.project {
        Some(name) => (name.as_str(), true),
        None => (repo.name.as_str(), false),
    };
    find(vault, name).ok_or_else(|| Unmapped {
        name: name.to_string(),
        overridden,
    })
}

pub fn find(vault: &Path, name: &str) -> Option<Project> {
    // An override is free text and must not reach outside the locations.
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return None;
    }
    let note = format!("{name}.md");
    LOCATIONS.iter().find_map(|loc| {
        let dir = vault.join(loc);
        // Listing rather than probing keeps a case-insensitive filesystem
        // from matching `Foo` to `foo`.
        if !has_entry(&dir, name, true) || !has_entry(&dir.join(name), &note, false) {
            return None;
        }
        Some(Project {
            name: name.to_string(),
            home: Path::new(loc).join(name).join(&note),
        })
    })
}

fn has_entry(dir: &Path, name: &str, want_dir: bool) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_name() == name
            && fs::metadata(entry.path()).is_ok_and(|meta| {
                if want_dir {
                    meta.is_dir()
                } else {
                    meta.is_file()
                }
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::repo::{Repo, Settings};

    fn home(vault: &Path, dir: &str, name: &str) -> PathBuf {
        let rel = Path::new(dir).join(name).join(format!("{name}.md"));
        let path = vault.join(&rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        rel
    }

    fn project(name: &str, home: PathBuf) -> Option<Project> {
        Some(Project {
            name: name.into(),
            home,
        })
    }

    fn repo(name: &str) -> Repo {
        Repo {
            root: PathBuf::from("/r").join(name),
            name: name.into(),
        }
    }

    fn settings(project: Option<&str>) -> Settings {
        Settings {
            prefix: "zz".into(),
            project: project.map(Into::into),
        }
    }

    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in fs::read_dir(&d).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(path);
            }
        }
        out.sort();
        out
    }

    #[test]
    fn each_location_is_found() {
        for loc in LOCATIONS {
            let vault = tempfile::tempdir().unwrap();
            let rel = home(vault.path(), loc, "a");
            let found = find(vault.path(), "a");
            assert_eq!(found, project("a", rel), "{loc}");
            assert_eq!(found.unwrap().folder(), Path::new(loc).join("a"), "{loc}");
        }
    }

    #[test]
    fn locations_are_tried_in_order() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        let homes: Vec<PathBuf> = LOCATIONS.iter().map(|loc| home(v, loc, "a")).collect();
        assert_eq!(find(v, "a"), project("a", homes[0].clone()));
        fs::remove_file(v.join(&homes[0])).unwrap();
        assert_eq!(find(v, "a"), project("a", homes[1].clone()));
        fs::remove_file(v.join(&homes[1])).unwrap();
        assert_eq!(find(v, "a"), project("a", homes[2].clone()));
    }

    #[test]
    fn folder_without_home_note_is_not_a_project() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        fs::create_dir_all(v.join("Projects/a")).unwrap();
        assert_eq!(find(v, "a"), None);
        fs::create_dir_all(v.join("Projects/a/a.md")).unwrap();
        assert_eq!(find(v, "a"), None);
    }

    #[test]
    fn case_mismatch_is_not_a_match() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        home(v, "Projects", "foo");
        assert_eq!(find(v, "Foo"), None);

        let other = tempfile::tempdir().unwrap();
        let o = other.path();
        fs::create_dir_all(o.join("Projects/Foo")).unwrap();
        fs::write(o.join("Projects/Foo/foo.md"), "").unwrap();
        assert_eq!(find(o, "Foo"), None);
    }

    #[test]
    fn path_like_names_match_nothing() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        home(v, "Areas", "x");
        home(v, "Projects/a", "b");
        fs::create_dir_all(v.join("Projects/a\\b")).unwrap();
        fs::write(v.join("Projects/a\\b/a\\b.md"), "").unwrap();
        for name in ["", ".", "..", "a/b", "../Areas/x", "a\\b"] {
            assert_eq!(find(v, name), None, "{name:?}");
        }
    }

    #[test]
    fn name_with_spaces() {
        let vault = tempfile::tempdir().unwrap();
        let rel = home(vault.path(), "Projects", "Example Project");
        assert_eq!(
            find(vault.path(), "Example Project"),
            project("Example Project", rel)
        );
    }

    #[test]
    fn map_uses_the_repo_name() {
        let vault = tempfile::tempdir().unwrap();
        let rel = home(vault.path(), "Projects", "ganymede");
        assert_eq!(
            map(vault.path(), &repo("ganymede"), &settings(None)).ok(),
            project("ganymede", rel)
        );
    }

    #[test]
    fn map_prefers_the_override() {
        let vault = tempfile::tempdir().unwrap();
        home(vault.path(), "Projects", "infra");
        let rel = home(vault.path(), "Projects", "Example Project");
        assert_eq!(
            map(
                vault.path(),
                &repo("infra"),
                &settings(Some("Example Project"))
            )
            .ok(),
            project("Example Project", rel)
        );
    }

    #[test]
    fn override_maps_dotfiles_to_an_area() {
        let vault = tempfile::tempdir().unwrap();
        let rel = home(vault.path(), "Areas", "dotfiles");
        assert_eq!(
            map(
                vault.path(),
                &repo(".dotfiles"),
                &settings(Some("dotfiles"))
            )
            .ok(),
            project("dotfiles", rel)
        );
    }

    #[test]
    fn missing_project_is_unmapped() {
        let vault = tempfile::tempdir().unwrap();
        let err = map(vault.path(), &repo("stctl"), &settings(None)).unwrap_err();
        assert_eq!(
            err,
            Unmapped {
                name: "stctl".into(),
                overridden: false,
            }
        );
        let text = err.to_string();
        assert!(
            text.contains(r#"no project or area named "stctl""#),
            "{text}"
        );
        assert!(text.contains("skipping"), "{text}");
        assert!(!text.contains("custom.obsidian.project"), "{text}");
    }

    #[test]
    fn missing_override_is_unmapped() {
        let vault = tempfile::tempdir().unwrap();
        home(vault.path(), "Projects", "infra");
        let err = map(vault.path(), &repo("infra"), &settings(Some("Gone"))).unwrap_err();
        assert_eq!(err.name, "Gone");
        assert!(err.overridden);
        assert!(err.to_string().contains("custom.obsidian.project"), "{err}");
    }

    #[test]
    fn map_never_creates_a_project() {
        let vault = tempfile::tempdir().unwrap();
        home(vault.path(), "Areas", "other");
        let before = files(vault.path());
        assert!(map(vault.path(), &repo("stctl"), &settings(None)).is_err());
        assert!(map(vault.path(), &repo("stctl"), &settings(Some("New"))).is_err());
        assert_eq!(files(vault.path()), before);
    }
}
