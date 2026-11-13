//! The README's examples MUST stay usable as written.

use std::fs;
use std::path::Path;

use callisto::config::{Config, Filing};

const OPTIONS: &[&str] = &[
    "enable",
    "package",
    "vault",
    "databases",
    "unfiled",
    "path",
    "startInterval",
    "onCalendar",
];

fn readme() -> String {
    fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap()
}

/// Every ```toml block that is a callisto config. A `[features]`
/// block is Codex's, which callisto does not read.
fn config_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    while lines.by_ref().any(|l| l == "```toml") {
        let block: Vec<_> = lines.by_ref().take_while(|l| *l != "```").collect();
        if !block.iter().any(|l| l.trim() == "[features]") {
            blocks.push(block.join("\n"));
        }
    }
    blocks
}

#[test]
fn readme_config_examples_load() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.toml");
    let blocks = config_blocks(&readme());
    assert!(blocks.len() >= 2, "{} config blocks", blocks.len());
    let mut configs = Vec::new();
    for block in &blocks {
        fs::write(&file, block).unwrap();
        let config =
            Config::load(&file, Some(Path::new("/h"))).unwrap_or_else(|e| panic!("{e}\n{block}"));
        assert!(!config.databases.is_empty(), "no databases in\n{block}");
        configs.push(config);
    }
    assert_eq!(configs[0].vault, Some("/h/org".into()));
    let split = configs
        .iter()
        .flat_map(|c| &c.databases)
        .any(|d| matches!(d.filing, Filing::Split(_)));
    assert!(split, "no split example");
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
