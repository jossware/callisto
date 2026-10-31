use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Vault root. Overrides the config file's vault.
    #[arg(long, global = true)]
    pub vault: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Mirror beads from the given repos into the vault.
    Sync(SyncArgs),
    /// Run as a Claude Code or Codex Stop hook. Always exits 0.
    ///
    /// Reads the hook payload on stdin and syncs the repo its cwd is in.
    /// Configure it as a `Stop` command hook running `callisto hook`, in
    /// ~/.claude/settings.json or ~/.codex/hooks.json.
    Hook,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Repo checkouts to sync. Defaults to the current directory's repo.
    #[arg(conflicts_with = "all")]
    pub repos: Vec<PathBuf>,
    /// Pull and sync every configured repo.
    #[arg(long)]
    pub all: bool,
}

/// Whether the subcommand is `hook`, judged before clap parses, since
/// `hook` MUST exit 0 even when its arguments are bad. Options before the
/// subcommand are stepped over with their values, so `--vault sync` is
/// not read as the subcommand.
pub fn is_hook(args: &[OsString]) -> bool {
    let cli = Cli::command();
    let takes_value = |flag: &str| {
        cli.get_arguments()
            .chain(cli.get_subcommands().flat_map(|sub| sub.get_arguments()))
            .filter(|arg| arg.get_action().takes_values())
            .any(|arg| {
                arg.get_long()
                    .is_some_and(|l| flag.strip_prefix("--") == Some(l))
                    || arg.get_short().is_some_and(|c| flag == format!("-{c}"))
            })
    };
    let mut rest = args.iter().skip(1).map(|a| a.to_string_lossy());
    while let Some(arg) = rest.next() {
        if !arg.starts_with('-') {
            return arg == "hook";
        }
        // `--vault hook` is either a value or a missing value before the
        // subcommand. Read it as the subcommand: failing the harness costs
        // more than mistaking a malformed manual run for a hook.
        if !arg.contains('=') && takes_value(&arg) && rest.next().is_some_and(|v| v == "hook") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("callisto").chain(args.iter().copied()))
    }

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn sync_takes_repo_paths() {
        let cli = parse(&["sync", "a", "b"]).unwrap();
        let Command::Sync(args) = cli.command else {
            panic!("expected sync")
        };
        assert_eq!(args.repos, [PathBuf::from("a"), PathBuf::from("b")]);
        assert!(!args.all);
    }

    #[test]
    fn sync_defaults_to_no_repos() {
        let cli = parse(&["sync"]).unwrap();
        let Command::Sync(args) = cli.command else {
            panic!("expected sync")
        };
        assert!(args.repos.is_empty());
    }

    #[test]
    fn all_conflicts_with_repos() {
        assert!(parse(&["sync", "--all"]).is_ok());
        let err = parse(&["sync", "--all", "a"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn spots_hook_before_parsing() {
        let args = |list: &[&str]| {
            std::iter::once("callisto")
                .chain(list.iter().copied())
                .map(OsString::from)
                .collect::<Vec<_>>()
        };
        for yes in [
            &["hook", "extra"][..],
            &["--bogus", "hook"],
            &["--vault", "/v", "hook"],
            &["--vault=/v", "hook"],
            &["--vault", "hook"],
            &["--vault", "sync", "hook"],
        ] {
            assert!(is_hook(&args(yes)), "{yes:?}");
        }
        for no in [
            &["sync", "hook"][..],
            &[],
            &["--vault", "/v", "sync", "hook"],
        ] {
            assert!(!is_hook(&args(no)), "{no:?}");
        }
    }

    #[test]
    fn vault_flag_is_global() {
        for args in [&["--vault", "/v", "hook"][..], &["hook", "--vault", "/v"]] {
            assert_eq!(parse(args).unwrap().vault, Some(PathBuf::from("/v")));
        }
        assert_eq!(parse(&["sync"]).unwrap().vault, None);
    }
}
