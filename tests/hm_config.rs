//! Loads the files the home-manager module generates. `checks.config-loader`
//! supplies them; the test is ignored everywhere else.

use std::path::{Path, PathBuf};

use callisto::config::{Config, RepoEntry, TableEntry};

fn table(path: &str, field: &str, value: Option<&[&str]>, project: Option<&str>) -> RepoEntry {
    RepoEntry::Table(TableEntry {
        path: PathBuf::from(path),
        field: field.to_string(),
        value: value.map(|v| v.iter().map(|s| s.to_string()).collect()),
        project: project.map(str::to_string),
    })
}

#[test]
#[ignore]
fn module_config_loads() {
    let dir = PathBuf::from(
        std::env::var_os("CALLISTO_HM_CONFIG_DIR").expect("CALLISTO_HM_CONFIG_DIR unset"),
    );
    let load = |name: &str| Config::load(&dir.join(name), Some(Path::new("/h"))).unwrap();

    assert_eq!(
        load("mixed.toml"),
        Config {
            vault: Some(PathBuf::from("/h/org")),
            field: Some("metadata.repo".to_string()),
            unfiled: PathBuf::from("Inbox/Beads"),
            databases: vec![],
            repos: vec![
                RepoEntry::Path(PathBuf::from("/h/Workspaces/callisto")),
                table(
                    "/h/work/p",
                    "metadata.repo",
                    Some(&["github.com/o/p"]),
                    Some("P")
                ),
            ],
        }
    );

    assert_eq!(
        load("tables-only.toml"),
        Config {
            vault: Some(PathBuf::from("/h/org")),
            field: Some("metadata.repo".to_string()),
            unfiled: PathBuf::from("Inbox"),
            databases: vec![],
            repos: vec![
                table("/srv/q", "x.y", Some(&["a", "b"]), None),
                table("/srv/r", "metadata.repo", None, None),
                table("/srv/s", "metadata.repo", Some(&["one"]), None),
            ],
        }
    );
}
