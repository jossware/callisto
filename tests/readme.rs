//! The README's examples MUST stay usable as written.

use std::fs;
use std::path::Path;

use callisto::config::Config;

const OPTIONS: &[&str] = &[
    "enable",
    "package",
    "vault",
    "repos",
    "path",
    "startInterval",
    "onCalendar",
];

fn readme() -> String {
    fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap()
}

fn first_block(text: &str, fence: &str) -> String {
    let mut lines = text.lines().skip_while(|l| *l != fence).skip(1);
    let block: Vec<_> = lines.by_ref().take_while(|l| *l != "```").collect();
    assert!(!block.is_empty(), "no {fence} block");
    block.join("\n")
}

#[test]
fn readme_config_example_loads() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.toml");
    fs::write(&file, first_block(&readme(), "```toml")).unwrap();
    let config = Config::load(&file, Some(Path::new("/h"))).unwrap();
    assert_eq!(config.vault, Some("/h/org".into()));
    assert!(!config.repos.is_empty());
}

#[test]
fn readme_module_options_exist() {
    let text = readme();
    let named: Vec<&str> = text
        .match_indices("services.callisto.")
        // systemd.user.services.callisto.* is home-manager's, not ours.
        .filter(|(i, _)| !text[..*i].ends_with('.'))
        .map(|(i, m)| {
            let rest = &text[i + m.len()..];
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            &rest[..end]
        })
        .collect();
    assert!(!named.is_empty());
    for name in named {
        assert!(OPTIONS.contains(&name), "unknown option {name}");
    }
}
