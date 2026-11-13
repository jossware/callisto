//! The vault project or area a database's notes are filed under.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::repo::{PROJECT_KEY, Settings};

/// Where a project or area's folder may live, in lookup order, unless
/// the config sets `locations`.
pub const DEFAULT_LOCATIONS: [&str; 4] =
    ["Projects", "Areas", "Archives/Projects", "Archives/Areas"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    /// Where new notes go, relative to the vault.
    pub folder: PathBuf,
}

/// A database with no project to file under. It is skipped, never given
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmapped {
    pub name: String,
    pub source: Source,
}

/// Where a project name came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The name of the directory that holds the database.
    Default,
    /// The database's bd setting.
    Setting,
    /// The database's `project` in the config file.
    Config,
}

impl fmt::Display for Unmapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = match self.source {
            Source::Default => String::new(),
            Source::Setting => format!(" (set by {PROJECT_KEY})"),
            Source::Config => " (set by project in config)".to_string(),
        };
        write!(
            f,
            "no project or area named \"{}\" in the vault{source}; skipping",
            self.name
        )
    }
}

/// Files under `name` unless `settings` overrides it. A set override is
/// final: falling back to `name` would hide a typo in it.
pub fn map(
    vault: &Path,
    locations: &[PathBuf],
    name: &str,
    settings: &Settings,
) -> Result<Project, Unmapped> {
    let (name, source) = match &settings.project {
        Some(project) => (project.as_str(), Source::Setting),
        None => (name, Source::Default),
    };
    find(vault, locations, name).ok_or_else(|| Unmapped {
        name: name.to_string(),
        source,
    })
}

pub fn find(vault: &Path, locations: &[PathBuf], name: &str) -> Option<Project> {
    // An override is free text and must not reach outside the locations.
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return None;
    }
    locations.iter().find_map(|loc| {
        // Listing rather than probing keeps a case-insensitive filesystem
        // from matching `Foo` to `foo`.
        if !is_dir_entry(&vault.join(loc), name) {
            return None;
        }
        Some(Project {
            name: name.to_string(),
            folder: loc.join(name),
        })
    })
}

fn is_dir_entry(dir: &Path, name: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_name() == name && fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::repo::Settings;

    fn dir(vault: &Path, loc: &str, name: &str) -> PathBuf {
        let rel = Path::new(loc).join(name);
        fs::create_dir_all(vault.join(&rel)).unwrap();
        rel
    }

    fn project(name: &str, folder: PathBuf) -> Option<Project> {
        Some(Project {
            name: name.into(),
            folder,
        })
    }

    fn defaults() -> Vec<PathBuf> {
        DEFAULT_LOCATIONS.iter().map(PathBuf::from).collect()
    }

    fn list(locs: &[&str]) -> Vec<PathBuf> {
        locs.iter().map(PathBuf::from).collect()
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
    fn each_default_location_is_found() {
        for loc in DEFAULT_LOCATIONS {
            let vault = tempfile::tempdir().unwrap();
            let rel = dir(vault.path(), loc, "a");
            let found = find(vault.path(), &defaults(), "a");
            assert_eq!(found, project("a", rel), "{loc}");
            assert_eq!(found.unwrap().folder, Path::new(loc).join("a"), "{loc}");
        }
    }

    #[test]
    fn default_locations_are_tried_in_order() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        let dirs: Vec<PathBuf> = DEFAULT_LOCATIONS
            .iter()
            .map(|loc| dir(v, loc, "a"))
            .collect();
        for (i, rel) in dirs.iter().enumerate() {
            assert_eq!(find(v, &defaults(), "a"), project("a", rel.clone()), "{i}");
            fs::remove_dir(v.join(rel)).unwrap();
        }
        assert_eq!(find(v, &defaults(), "a"), None);
    }

    #[test]
    fn configured_locations_are_tried_in_order() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        let locs = list(&["X", "Y/Z"]);
        let x = dir(v, "X", "a");
        let yz = dir(v, "Y/Z", "a");
        assert_eq!(find(v, &locs, "a"), project("a", x.clone()));
        fs::remove_dir(v.join(&x)).unwrap();
        assert_eq!(find(v, &locs, "a"), project("a", yz));
    }

    #[test]
    fn folder_outside_the_list_is_not_found() {
        let vault = tempfile::tempdir().unwrap();
        dir(vault.path(), "Projects", "a");
        assert_eq!(find(vault.path(), &list(&["X"]), "a"), None);
    }

    #[test]
    fn folder_without_note_is_a_project() {
        let vault = tempfile::tempdir().unwrap();
        let rel = dir(vault.path(), "Projects", "a");
        assert_eq!(find(vault.path(), &defaults(), "a"), project("a", rel));
    }

    #[test]
    fn file_named_like_project_is_skipped() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        fs::create_dir_all(v.join("X")).unwrap();
        fs::write(v.join("X/a"), "").unwrap();
        let rel = dir(v, "Y", "a");
        assert_eq!(find(v, &list(&["X", "Y"]), "a"), project("a", rel));
    }

    #[test]
    fn missing_location_is_skipped() {
        let vault = tempfile::tempdir().unwrap();
        let rel = dir(vault.path(), "Y", "a");
        assert_eq!(
            find(vault.path(), &list(&["Gone", "Y"]), "a"),
            project("a", rel)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folder_is_found() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        fs::create_dir_all(v.join("elsewhere/a")).unwrap();
        fs::create_dir_all(v.join("Projects")).unwrap();
        std::os::unix::fs::symlink(v.join("elsewhere/a"), v.join("Projects/a")).unwrap();
        assert_eq!(
            find(v, &defaults(), "a"),
            project("a", PathBuf::from("Projects/a"))
        );
    }

    #[test]
    fn case_mismatch_is_not_a_match() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        dir(v, "Projects", "foo");
        assert_eq!(find(v, &defaults(), "Foo"), None);

        let other = tempfile::tempdir().unwrap();
        let o = other.path();
        dir(o, "Projects", "Foo");
        assert_eq!(find(o, &defaults(), "foo"), None);
    }

    #[test]
    fn path_like_names_match_nothing() {
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path();
        dir(v, "Areas", "x");
        dir(v, "Projects/a", "b");
        dir(v, "Projects", "a\\b");
        for name in ["", ".", "..", "a/b", "../Areas/x", "a\\b"] {
            assert_eq!(find(v, &defaults(), name), None, "{name:?}");
        }
    }

    #[test]
    fn name_with_spaces() {
        let vault = tempfile::tempdir().unwrap();
        let rel = dir(vault.path(), "Projects", "Example Project");
        assert_eq!(
            find(vault.path(), &defaults(), "Example Project"),
            project("Example Project", rel)
        );
    }

    #[test]
    fn map_uses_the_name() {
        let vault = tempfile::tempdir().unwrap();
        let rel = dir(vault.path(), "Projects", "ganymede");
        assert_eq!(
            map(vault.path(), &defaults(), "ganymede", &settings(None)).ok(),
            project("ganymede", rel)
        );
    }

    #[test]
    fn map_uses_the_configured_locations() {
        let vault = tempfile::tempdir().unwrap();
        dir(vault.path(), "Projects", "ganymede");
        let rel = dir(vault.path(), "Work", "ganymede");
        assert_eq!(
            map(vault.path(), &list(&["Work"]), "ganymede", &settings(None)).ok(),
            project("ganymede", rel)
        );
    }

    #[test]
    fn map_prefers_the_override() {
        let vault = tempfile::tempdir().unwrap();
        dir(vault.path(), "Projects", "infra");
        let rel = dir(vault.path(), "Projects", "Example Project");
        assert_eq!(
            map(
                vault.path(),
                &defaults(),
                "infra",
                &settings(Some("Example Project"))
            )
            .ok(),
            project("Example Project", rel)
        );
    }

    #[test]
    fn override_maps_dotfiles_to_an_area() {
        let vault = tempfile::tempdir().unwrap();
        let rel = dir(vault.path(), "Areas", "dotfiles");
        assert_eq!(
            map(
                vault.path(),
                &defaults(),
                ".dotfiles",
                &settings(Some("dotfiles"))
            )
            .ok(),
            project("dotfiles", rel)
        );
    }

    #[test]
    fn missing_project_is_unmapped() {
        let vault = tempfile::tempdir().unwrap();
        let err = map(vault.path(), &defaults(), "stctl", &settings(None)).unwrap_err();
        assert_eq!(
            err,
            Unmapped {
                name: "stctl".into(),
                source: Source::Default,
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
        dir(vault.path(), "Projects", "infra");
        let err = map(vault.path(), &defaults(), "infra", &settings(Some("Gone"))).unwrap_err();
        assert_eq!(err.name, "Gone");
        assert_eq!(err.source, Source::Setting);
        assert!(err.to_string().contains("custom.obsidian.project"), "{err}");
    }

    #[test]
    fn map_never_creates_a_project() {
        let vault = tempfile::tempdir().unwrap();
        dir(vault.path(), "Areas", "other");
        let before = files(vault.path());
        let d = defaults();
        assert!(map(vault.path(), &d, "stctl", &settings(None)).is_err());
        assert!(map(vault.path(), &d, "stctl", &settings(Some("New"))).is_err());
        assert_eq!(files(vault.path()), before);
    }
}
