//! Loads the files the home-manager module generates. `checks.config-loader`
//! supplies them; the test is ignored everywhere else.

use std::path::{Path, PathBuf};

use callisto::config::Config;

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
            unfiled: PathBuf::from("Inbox/Beads"),
            databases: vec![],
        }
    );
}
