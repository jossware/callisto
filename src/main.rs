use std::env;
use std::io;
use std::process::ExitCode;

use clap::Parser;
use clap::error::ErrorKind;

use callisto::bd::Bd;
use callisto::cli::{self, Cli, Command};
use callisto::cmd;
use callisto::paths::Env;

fn main() -> ExitCode {
    let env = Env::read();
    let args: Vec<_> = env::args_os().collect();
    let hook = cli::is_hook(&args);
    if hook {
        cmd::hook::install_panic_hook(env.clone());
    }
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e)
            if hook && !matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) =>
        {
            let message = e.to_string();
            let first = message.lines().next().unwrap_or("bad arguments");
            cmd::hook::bad_args(first, &env, &mut io::stderr());
            return ExitCode::SUCCESS;
        }
        Err(e) => e.exit(),
    };
    match cli.command {
        Command::Sync(sync) => {
            let args = cmd::sync::Args {
                repos: &sync.repos,
                all: sync.all,
                vault: cli.vault.as_deref(),
            };
            // An unreadable cwd leaves relative paths unresolvable, and
            // each then fails on its own as not in a repository.
            let cwd = env::current_dir().unwrap_or_default();
            cmd::sync::run(
                &args,
                &env,
                &Bd::default(),
                &cwd,
                &mut io::stdout(),
                &mut io::stderr(),
            )
        }
        Command::Hook => {
            cmd::hook::run(
                cli.vault.as_deref(),
                &env,
                &Bd::default(),
                &mut io::stdin(),
                &mut io::stderr(),
            );
            ExitCode::SUCCESS
        }
    }
}
