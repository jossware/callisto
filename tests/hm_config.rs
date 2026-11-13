//! Loads the files the home-manager module generates. `checks.config-loader`
//! supplies them; the test is ignored everywhere else.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use callisto::config::{Config, DatabaseEntry, Filing, Split};

#[test]
#[ignore]
fn module_config_loads() {
    let dir = PathBuf::from(
        std::env::var_os("CALLISTO_HM_CONFIG_DIR").expect("CALLISTO_HM_CONFIG_DIR unset"),
    );
    let load = |name: &str| Config::load(&dir.join(name), Some(Path::new("/h"))).unwrap();

    assert_eq!(
        load("single.toml"),
        Config {
            vault: Some(PathBuf::from("/h/org")),
            unfiled: PathBuf::from("Inbox"),
            databases: vec![
                DatabaseEntry {
                    beads_dir: PathBuf::from("/h/Workspaces/callisto/.beads"),
                    filing: Filing::Single { project: None },
                },
                DatabaseEntry {
                    beads_dir: PathBuf::from("/h/.dotfiles/.beads"),
                    filing: Filing::Single {
                        project: Some("dotfiles".into()),
                    },
                },
            ],
        }
    );

    assert_eq!(
        load("split.toml"),
        Config {
            vault: Some(PathBuf::from("/h/org")),
            unfiled: PathBuf::from("Inbox/Beads"),
            databases: vec![
                DatabaseEntry {
                    beads_dir: PathBuf::from("/srv/tasksdb/.beads"),
                    filing: Filing::Split(Split {
                        by: "metadata.repo".parse().unwrap(),
                        projects: BTreeMap::from([
                            ("github.com/o/api".into(), "API".into()),
                            ("github.com/o/web".into(), "Web".into()),
                        ]),
                    }),
                },
                DatabaseEntry {
                    beads_dir: PathBuf::from("/srv/other/.beads"),
                    filing: Filing::Split(Split {
                        by: "x.y".parse().unwrap(),
                        projects: BTreeMap::new(),
                    }),
                },
            ],
        }
    );
}
