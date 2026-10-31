# callisto

callisto mirrors [beads](https://github.com/gastownhall/beads) issues
into an Obsidian vault. Each bead becomes one Markdown note in the
TaskNotes format, filed under its project, so beads show up in the
vault's task views, link to daily notes, and sync to mobile.

The vault is a read-only mirror. Beads is the only source of truth.

- callisto writes only notes it generated. They carry
  `generatedBy: callisto`. Every other note, including hand-written
  notes with a `bead:` field, is never modified, moved, or deleted.
- A rewrite replaces the whole note, so hand edits to a generated note
  are lost.
- Output is deterministic. A sync writes only notes whose content
  changed.
- A note outlives its bead. When a bead disappears from beads, its note
  is kept, marked `beadRemoved: true`, and set to `dropped` unless it
  was already `done` or `dropped`. Each note records its repo's bead
  prefix in `beadPrefix`, and only a sync of that repo marks it. After
  a prefix rename, old notes are no longer marked; delete or mark them
  by hand.

The
[Force note view mode](https://github.com/bwydoogh/obsidian-force-view-mode-of-note)
plugin (`obsidian-force-view-mode-of-note`) is recommended. Generated
notes carry `obsidianUIMode: preview`, and the plugin opens them in
Reading view.
Live Preview mis-measures a tall callout full of TaskNotes widgets and
draws the panels below the note over it; the notes are read-only
anyway. Without the plugin the field does nothing.

It is a sibling of [ganymede](https://github.com/jossware/ganymede),
which archives agent sessions into the same vault.

## Install

callisto is packaged as a Nix flake:

```sh
nix profile install github:jossware/callisto
```

Or add it as a flake input and use `packages.<system>.default`, or the
home-manager module (see [Scheduled sync](#scheduled-sync)):

```nix
inputs.callisto = {
  url = "github:jossware/callisto";
  inputs.nixpkgs.follows = "nixpkgs";
  inputs.home-manager.follows = "home-manager";
};
```

From a checkout, run `nix build` or `nix profile install .`.

The flake builds for `aarch64-darwin`, `aarch64-linux` and
`x86_64-linux`; nixpkgs no longer evaluates `x86_64-darwin`. On any
other system, build callisto yourself and set
`services.callisto.package`.

callisto runs `bd` (1.3.1 or later) for all beads access. `bd` MUST be on
`PATH` wherever callisto runs.

## Configuration

callisto reads one file:

```
$XDG_CONFIG_HOME/callisto/config.toml
~/.config/callisto/config.toml    # when XDG_CONFIG_HOME is unset
```

```toml
vault = "~/org"
repos = [
  "~/Workspaces/callisto",
  "~/Workspaces/ganymede",
]
```

| Key | |
|---|---|
| `vault` | Vault root. Required here or by `--vault`, which overrides it. |
| `repos` | Repo checkouts that `sync --all` syncs. Every command also uses the list to tell which repo owns a bead id in an export, or in a note without `beadPrefix`. |

- Entries MUST be absolute or start with `~/`. The scheduled job runs
  in no particular directory, so relative entries are rejected.
- Unknown keys are an error, so a typo cannot empty the list.
- An absent file is an empty list. `sync <path>` and the hook still
  work, with a warning that the repo's prefix is unchecked.
  `sync --all` fails.

### Projects

Each repo maps to a project or area in the vault:

1. The repo's `custom.obsidian.project` setting, if set:

   ```sh
   bd config set custom.obsidian.project "Example Project"
   ```

2. Otherwise, the name of the repo's main checkout directory.

The project's home note is looked up at `Projects/<Name>/<Name>.md`,
`Areas/<Name>/<Name>.md`, `Archives/Projects/<Name>/<Name>.md`, then
`Archives/Areas/<Name>/<Name>.md`. New notes go in that folder, named
`<title> - <bead-id>.md`. A repo with no project is skipped with a
warning. callisto never creates a project.

## Commands

The global flag `--vault <dir>` overrides the config file's vault.

### `callisto sync [<repo>...]`

Syncs the given repo checkouts, by default the current directory's
repo. A worktree or jj workspace resolves to its main checkout. Prints
one line per repo:

```
ganymede: 2 created, 5 updated, 0 removed, 40 unchanged
```

Warnings go to stderr. Exits non-zero if any repo failed.

### `callisto sync --all`

Runs `bd dolt pull` in every configured repo, then syncs them. A repo
whose pull fails is logged and skipped; the others still sync. This is
what the scheduled job runs.

### `callisto hook`

A Claude Code and Codex `Stop` hook. It reads the hook payload on stdin
and syncs the repo its `cwd` is in, so notes catch up as soon as an
agent finishes a turn. It does not pull.

The hook always exits 0, and its stdout is always empty. It does
nothing when the directory has no `.beads/`, when no vault is set,
when the vault directory is absent, or when another sync holds the
lock. Failures go to the error log only.

## Hook setup

The hook is optional. It speeds up the designated host; the scheduled
sync is enough for correctness. Machines without the vault MAY skip it.

### Claude Code

Add the hook to `~/.claude/settings.json`. Claude Code shows
`statusMessage` while the hook runs.

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "callisto hook",
            "statusMessage": "Syncing beads to the vault"
          }
        ]
      }
    ]
  }
}
```

### Codex

Hooks MUST be enabled in `~/.codex/config.toml`:

```toml
[features]
hooks = true
```

Then add callisto to `~/.codex/hooks.json`. Keep any entries already
there.

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "callisto hook",
            "statusMessage": "Syncing beads to the vault"
          }
        ]
      }
    ]
  }
}
```

Codex shows `statusMessage` while the hook runs. Adding it to an
approved hook changes the hook, so Codex asks for approval again.

Codex does not run a new hook until you approve it. Start the Codex
TUI, approve the hook when prompted, and finish one turn.

## Scheduled sync

The primary trigger is a scheduled `callisto sync --all` on the
designated host: the machine that holds the vault and whose beads
databases you keep pulled. The flake exports a home-manager module that
installs callisto and schedules it: a launchd agent on macOS, a systemd
user timer on Linux.

Enable it on one host only. A host without the vault would fail and
log every run.

```nix
{ inputs, ... }:
{
  imports = [ inputs.callisto.homeManagerModules.default ];

  services.callisto = {
    enable = true;
    vault = "~/org";
    repos = [
      "~/Workspaces/callisto"
      "~/Workspaces/ganymede"
    ];
  };
}
```

| Option | Default | |
|---|---|---|
| `services.callisto.enable` | `false` | Install callisto and schedule `sync --all`. |
| `services.callisto.package` | this flake's | The package to install and run. |
| `services.callisto.vault` | `null` | Written to `config.toml`. |
| `services.callisto.repos` | `[ ]` | Written to `config.toml`. Each MUST be absolute or start with `~/`. |
| `services.callisto.path` | profile `bin`, then system dirs | `PATH` for the job. |
| `services.callisto.startInterval` | `900` | Seconds between launchd runs (macOS). |
| `services.callisto.onCalendar` | `"*:0/15"` | systemd timer schedule (Linux). |

Set `repos` through the module. Once `vault` or `repos` is set,
home-manager owns `config.toml`. `sync --all` fails without repos or
a vault, so every switch warns while `repos` is empty or `vault` is
unset.

A hand-written `config.toml` is discouraged. With `vault` and `repos`
both unset, the module leaves the file alone and the job reads it, but
every switch warns unless the module sets `vault` and `repos` or the
file comes from `xdg.configFile."callisto/config.toml"`.

What each platform gets:

- **macOS:** a launchd agent labeled `org.nix-community.home.callisto`.
  It runs at load and every `startInterval` seconds. Its output is
  discarded; failures are in the error log.
- **Linux:** `callisto.service` (oneshot) and `callisto.timer`. The
  timer is persistent, so a run missed while asleep happens on wake.
  Output goes to the journal.

To preview the units for your platform without installing anything:

```sh
nix build github:jossware/callisto#units
```

Things to know:

- `bd dolt pull` runs unattended. Its remote MUST be reachable without
  a prompt: a key in the agent, no passphrase dialog.
- On Linux, the systemd user manager does not inherit your shell's
  `SSH_AUTH_SOCK`, so the job cannot reach the agent you `ssh-add` to.
  Use an agent whose socket has a fixed path, and point the job at it
  (`%t` is `$XDG_RUNTIME_DIR`). home-manager's `services.ssh-agent`
  listens at `%t/ssh-agent`, gpg-agent at `%t/gnupg/S.gpg-agent.ssh`,
  gnome-keyring at `%t/keyring/ssh`:

  ```nix
  systemd.user.services.callisto.Service.Environment = [
    "SSH_AUTH_SOCK=%t/ssh-agent"
  ];
  ```

  An agent started with `eval $(ssh-agent)` has a random socket path
  that no unit can name. For one, run
  `systemctl --user import-environment SSH_AUTH_SOCK` in each session.
  Or give the remote a key that needs no agent, such as a deploy key
  without a passphrase.
- The job's `PATH` MUST reach `bd` and what `bd dolt pull` calls (git,
  ssh). The default is, in order: your home-manager profile
  (`home.profileDirectory`), `/run/current-system/sw/bin` (NixOS,
  nix-darwin), `/nix/var/nix/profiles/default/bin` (the nix-daemon's
  default profile), then the system dirs. Anything elsewhere, such as
  `~/.nix-profile/bin` under the NixOS or nix-darwin module with
  `home-manager.useUserPackages`, needs `services.callisto.path`.
- The job runs with home-manager's `xdg.configHome` and
  `xdg.stateHome`, so it shares the hook's config, lock and error log.
  Your shell MUST export the same `XDG_CONFIG_HOME` and
  `XDG_STATE_HOME` (or neither, with the defaults).
- The agent belongs in the `gui` launchd domain: agents in the `user`
  domain are not restored after a reboot
  ([home-manager#9656](https://github.com/nix-community/home-manager/issues/9656)).
  Current home-manager defaults to `gui`. To pin it:

  ```nix
  launchd.agents.callisto.domain = "gui";
  ```

Check that it runs:

```sh
# macOS
launchctl print "gui/$(id -u)/org.nix-community.home.callisto"

# Linux
systemctl --user list-timers callisto.timer
journalctl --user -u callisto.service
```

## Errors

Failures are logged, one JSON object per line:

```
$XDG_STATE_HOME/callisto/errors.jsonl
~/.local/state/callisto/errors.jsonl    # when XDG_STATE_HOME is unset
```

```json
{"ts":"…","version":"…","command":"sync","repo":"/Users/me/Workspaces/a","stage":"pull","error":"…"}
```

`stage` is one of `config`, `payload`, `lock`, `pull`, `export`,
`vault`, `render`, `write`, or `panic`. The log is created `0600`,
since entries carry paths and `bd` output. It is kept under 1 MiB by
dropping its older half.

The same directory holds `sync.lock`. A sync holds it from before
`bd dolt pull` until its last write, so a run that read stale beads
data cannot land after a fresher one. `sync` waits for it; the hook
skips its run instead.

## Development

Work inside the devShell. With direnv, `cd` into the repository.
Without it, use `nix develop -c <command>`.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
nix build
nix flake check
```

CI runs the same checks. `nix flake check` evaluates the home-manager
module for every supported system and validates every platform's
rendered units, so a Linux run checks the macOS plist too. Its
`trace: warning: services.callisto…` lines come from the tests of that
warning and are expected.

Renderer tests use [insta](https://insta.rs) snapshots. Review changes
with `cargo insta review`.

## License

[MIT](LICENSE)
