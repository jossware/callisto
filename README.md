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
  was already `done` or `dropped`. Each note records its bead's prefix
  in `beadPrefix`, and only a sync of a database with that prefix
  marks it.
  After a prefix rename, old notes are no longer marked; delete or mark
  them by hand.

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

callisto runs `bd` (1.3.4 or later) for all beads access. `bd` MUST be on
`PATH` wherever callisto runs.

## Configuration

callisto reads one file:

```
$XDG_CONFIG_HOME/callisto/config.toml
~/.config/callisto/config.toml    # when XDG_CONFIG_HOME is unset
```

It lists beads databases, each named by its directory. Where each
repo has a database of its own:

```toml
vault = "~/org"

[[databases]]
beads_dir = "~/Workspaces/callisto/.beads"

[[databases]]
beads_dir = "~/.dotfiles/.beads"
project = "dotfiles"
```

Where many repos share one database, each bead names its repo in a
field, and `split` files it by that field:

```toml
vault = "~/org"
unfiled = "Inbox/Beads"

[[databases]]
beads_dir = "~/tasksdb/.beads"
split.by = "metadata.repo"
split.projects = { "github.com/example/api" = "API" }
```

The shared database needs no clone of any repo.

| Key | |
|---|---|
| `vault` | Vault root. Required here or by `--vault`, which overrides it. |
| `unfiled` | Vault folder for the index of unfiled beads. Default `Inbox`. |
| `databases` | The databases `sync --all` syncs, in order. |

Each `[[databases]]` entry:

| Key | |
|---|---|
| `beads_dir` | The database directory. Required. |
| `project` | Every bead in the database goes to this project. |
| `split.by` | Dotted path into the bead as `bd export` emits it, such as `metadata.repo`. Required when `split` is set. |
| `split.projects` | Field value to project name. Optional. |

- `vault` and every `beads_dir` MUST be absolute or start with `~/`.
  The scheduled job runs in no particular directory, so relative
  entries are rejected.
- An entry MUST NOT set both `project` and `split`.
- Two entries MUST NOT name the same directory, after symlinks are
  resolved.
- `split.by` segments MUST NOT be empty.
- `unfiled` MUST be relative to the vault, with no empty, `.` or `..`
  segment.
- Unknown keys are an error, inside an entry and inside `split` too,
  so a typo cannot empty the list.
- A `beads_dir` that does not exist does not fail loading. It fails
  that database's sync; other databases still sync.
- An absent file is an empty list. `sync <path>` and the hook still
  work for a checkout with its own `.beads/`. `sync --all` fails.

Every `bd` call for a database runs in its directory, with
`BEADS_DIR` set to it and `BEADS_DB` removed. A `BEADS_DIR` or
`BEADS_DB` in your environment never changes what a configured
database reads.

### Projects

A database with no `split` files every bead under one project:

1. its `project`, if set;
2. else its `custom.obsidian.project` bd setting:

   ```sh
   bd config set custom.obsidian.project "Example Project"
   ```

3. else the name of the directory that holds `beads_dir`. So
   `~/.dotfiles/.beads` defaults to `.dotfiles`.

A split database files each bead by the value at `split.by`:

1. the value's entry in `split.projects`;
2. else the value's last `/`-separated segment, ignoring a trailing
   `/`. So `github.com/example/web` files under `web`.

Only a string value names a project. A missing field, a number, a
list, an object or `null` sends the bead to the
[unfiled index](#unfiled-beads). Values are compared exactly.
`custom.obsidian.project` is not consulted: repos that share a
database share its settings.

A project's home note is looked up at `Projects/<Name>/<Name>.md`,
`Areas/<Name>/<Name>.md`, `Archives/Projects/<Name>/<Name>.md`, then
`Archives/Areas/<Name>/<Name>.md`. New notes go in that folder, named
`<title> - <bead-id>.md`. callisto never creates a project.

- A single-project database whose project is not in the vault is
  skipped with a warning. A set `project` or `custom.obsidian.project`
  is final; callisto does not fall back to the directory name.
- In a split database, a bead whose project is not in the vault is
  unfiled, with the missing name as its reason.

A note is found by its bead id wherever it lies in the vault, and
updated there. callisto never moves a note. A bead whose project
changes keeps its note where it is; only its `projects:` link changes.

### Finding a directory's database

`sync <path>`, plain `sync` and the hook each start from a directory.
callisto finds its checkout. A worktree or jj workspace resolves to its
main checkout. It then runs `bd context --json` in the main checkout,
with your environment minus `BEADS_DB`, and uses the configured
database whose `beads_dir` matches the one reported. So a checkout with
no `.beads/` of its own reaches the database `$BEADS_DIR` names.

A directory in no checkout has no database: `sync` fails it as
`not in a repository`, and the hook does nothing. `$BEADS_DIR` alone
is never enough.

A database that is not configured:

- If it is the checkout's own `.beads/`, it syncs as a single-project
  database with defaults, with a warning that its prefix is unchecked
  against the configured databases.
- Otherwise it came from `$BEADS_DIR`, and has no meaningful name.
  `sync` reports `database <dir> is not configured` and exits
  non-zero. The hook skips it without logging.

### Prefixes and removal marking

Each sync exports the whole database. A note is marked removed when
its bead carries the database's prefix and is absent from the export.
callisto reads the prefix of every configured database on every run, to
tell which database owns a note.

- Two configured databases with the same prefix are a standing
  condition. Neither marks notes removed while it lasts. Both still
  create and update notes.
- While any configured database cannot be read (its directory is
  missing, or its prefix cannot be read), no sync marks notes removed.
- A database that is not configured but uses a configured database's
  prefix, such as a second clone of a configured repo, still creates
  and updates notes. It never marks notes removed, and warns why.

### Unfiled beads

In a split database, a bead is unfiled when its value names no
project, or names one that is not in the vault.

- An unfiled bead gets no note. A note it already has is left as it is.
  It is not marked removed, since the bead is still in beads.
- Once it can be filed, the next sync creates its note.

Each split database has one index note:

```
<unfiled>/Unfiled beads - <prefix>.md
```

It lists each unfiled bead that would otherwise get a note: id, title,
status and reason. Closed beads come after the rest. The reason is
that the field is absent, the value found (JSON for a value that is
not a string), or the missing project's name. callisto updates the
index on every sync, to an empty list when nothing is unfiled. It
never deletes it. Like a bead's note, the index carries
`generatedBy: callisto`, and hand edits are lost.

callisto never creates the unfiled folder. If the folder is missing, or
the index path holds a file callisto did not write, no index is
written. A single-project database has no index.

Each of these is a standing condition, logged as described in
[Errors](#errors): unfiled beads, a missing project, a missing unfiled
folder, an index path callisto did not write, a shared prefix, and a
database that cannot be read. `sync` exits non-zero for all but
unfiled beads and a missing project.

## Commands

The global flag `--vault <dir>` overrides the config file's vault.

### `callisto sync [<path>...]`

Syncs the database of each given directory, by default the current
directory's. See [Finding a directory's database](#finding-a-directorys-database).
Paths that resolve to one database sync it once. Prints one line per
database, named after the directory that holds its `beads_dir`:

```
ganymede: 2 created, 5 updated, 0 removed, 40 unchanged
```

Warnings and standing conditions go to stderr. Exits non-zero if any
database failed, or on a condition listed under
[Unfiled beads](#unfiled-beads).

### `callisto sync --all`

Runs `bd dolt pull` once per configured database, then syncs each, in
config order. A database whose directory is missing, whose pull fails,
or whose prefix cannot be read is logged and skipped; the others still
sync. With no databases configured, it fails. This is what the
scheduled job runs.

### `callisto hook`

A Claude Code and Codex `Stop` hook. It reads the hook payload on stdin
and syncs the database of the directory its `cwd` is in, so notes catch
up as soon as an agent finishes a turn. It does not pull.

The hook always exits 0, and its stdout is always empty. It does
nothing when the directory's checkout has no `.beads/` and `BEADS_DIR`
is unset, when its database is not configured and not the checkout's
own, when no vault is set, when the vault directory is absent, or when
another sync holds the lock. Failures go to the error log only.

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
    databases = [
      { beads_dir = "~/Workspaces/callisto/.beads"; }
      {
        beads_dir = "~/.dotfiles/.beads";
        project = "dotfiles";
      }
      {
        beads_dir = "~/work/tasksdb/.beads";
        split.by = "metadata.repo";
        split.projects."github.com/example/api" = "API";
      }
    ];
  };
}
```

| Option | Default | |
|---|---|---|
| `services.callisto.enable` | `false` | Install callisto and schedule `sync --all`. |
| `services.callisto.package` | this flake's | The package to install and run. |
| `services.callisto.vault` | `null` | Written to `config.toml`. |
| `services.callisto.databases` | `[ ]` | Written to `config.toml`. Each entry has `beads_dir`, and optionally `project` or `split` (`by`, `projects`). Each `beads_dir` MUST be absolute or start with `~/`. |
| `services.callisto.unfiled` | `null` | Written to `config.toml`. Unset, callisto uses `Inbox`. |
| `services.callisto.path` | profile `bin`, then system dirs | `PATH` for the job. |
| `services.callisto.startInterval` | `900` | Seconds between launchd runs (macOS). |
| `services.callisto.onCalendar` | `"*:0/15"` | systemd timer schedule (Linux). |

Set `databases` through the module. Once `vault` or `databases` is
set, home-manager owns `config.toml`. `sync --all` fails without
databases or a vault, so every switch warns while `databases` is empty
or `vault` is unset. `unfiled` alone does not make home-manager own
the file; a switch warns that it has no effect.

The module checks what it can before a switch: every `beads_dir` is
anchored, no entry sets both `project` and `split`, and `split.by` has
no empty segment.

A hand-written `config.toml` is discouraged. With `vault` and
`databases` both unset, the module leaves the file alone and the job
reads it, but every switch warns unless the module sets `vault` and
`databases` or the file comes from
`xdg.configFile."callisto/config.toml"`.

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
{"ts":"…","version":"…","command":"sync","repo":"/Users/me/Workspaces/a/.beads","stage":"pull","error":"…"}
```

`stage` is one of `config`, `payload`, `lock`, `pull`, `export`,
`vault`, `render`, `write`, `conditions`, or `panic`. The log is created
`0600`, since entries carry paths and `bd` output. It is kept under
1 MiB by dropping its older half.

Standing conditions are problems that persist across runs: unfiled
beads, a missing project, a missing unfiled folder, an index path
callisto did not write, a shared prefix, a database that cannot be
read. `sync` prints them on every run. The log gets them once when
they appear or change, and once when they clear.
`conditions.json` beside the log keeps the last logged summary per
subject, `0600`. Deleting it only re-logs current conditions.

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
rendered units, so a Linux run checks the macOS plist too. It also
loads the module's generated `config.toml` through callisto's config
loader. Its `trace: warning: services.callisto…` lines come from the
tests of that warning and are expected.

Renderer tests use [insta](https://insta.rs) snapshots. Review changes
with `cargo insta review`.

## License

[MIT](LICENSE)
