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
  in `beadPrefix`, and only a sync of a repo with that prefix marks it.
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
| `repos` | Repo checkouts that `sync --all` syncs. Each is a path or a table entry. Every command also uses the list to tell which repo owns a bead id in an export, or in a note without `beadPrefix`. |
| `field` | The field table entries match, unless an entry sets its own. No default. |
| `unfiled` | Vault folder for the index of unclaimed beads. Default `Inbox`. |

- `vault` and every repo path MUST be absolute or start with `~/`. The
  scheduled job runs in no particular directory, so relative entries
  are rejected.
- `unfiled` MUST be relative to the vault, with no empty, `.` or `..`
  segment.
- Unknown keys are an error, inside a table entry too, so a typo cannot
  empty the list.
- An absent file is an empty list. `sync <path>` and the hook still
  work, with a warning that the repo's prefix is unchecked.
  `sync --all` fails.

### Table entries

A path entry claims every bead in its repo's database. That fits a
repo with a database of its own. When several repos share one
database, and so one prefix, each bead can name its repo in a field.
A table entry claims beads by that field:

```toml
vault = "~/org"
field = "metadata.repo"
unfiled = "Inbox/Beads"
repos = [
  "~/Workspaces/callisto",
  { path = "~/work/api", value = "github.com/example/api", project = "API" },
  { path = "~/work/web", value = ["github.com/example/web", "github.com/example/www"] },
  { path = "~/work/tools" },
]
```

Here callisto has a database of its own. The three work repos share
another.

| Key | |
|---|---|
| `path` | Repo checkout. Required. |
| `value` | A string, or a list of strings. Default: the checkout's `origin` remote as `host/path`. |
| `field` | Dotted path to the field to match. Overrides the top-level `field`. One of the two MUST be set. |
| `project` | Vault project. Default: the name of the repo's main checkout directory. |

A table entry claims a bead when the bead carries the database's prefix
AND its `field` equals one of the entry's values:

- `field` is any dotted path into the bead as `bd export` emits it,
  such as `metadata.repo`. Segments MUST NOT be empty.
- Only a string value matches. An absent field, a number, a list, an
  object or `null` never does.
- Values are compared exactly. The bead's value is not normalised.
- A list claims a repo under several names, such as its old and new
  names after a rename.

When `value` is unset, callisto runs `git remote get-url origin` in the
checkout and normalises the URL to `host/path`. SSH and HTTPS forms
and a trailing `.git` normalise alike. The host is lowercased, any user
and port are dropped, and every path segment is kept. So
`git@github.com:example/api.git` and
`https://github.com/example/api` both give `github.com/example/api`.

- An SSH host alias is taken literally: `work:example/api` gives
  `work/example/api`. Such a repo MUST set `value`.
- A jj checkout without a colocated git repo has no `origin`. It MUST
  set `value`.
- `git` MUST be on `PATH` wherever callisto runs, when any table entry
  omits `value`.

callisto resolves the default on each sync, not when it loads the
config. An entry it cannot resolve does not stop other repos syncing;
see [Unclaimed beads](#unclaimed-beads).

### Projects

Each path entry maps to a project or area in the vault:

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

A table entry's project is its `project`, else the name of its main
checkout directory. `custom.obsidian.project` is not consulted: it is
stored in the database, so repos that share one share it. A table
entry whose project is not in the vault does not skip its repo. Its
beads are listed as unclaimed.

A note is found by its bead id wherever it lies in the vault, and
updated there. callisto never moves a note.

### Shared databases

Repos whose `.beads/` redirect to one database share its beads and its
prefix. callisto groups the configured repos by the database
`bd context` reports; you never list groups. A repo with a database of
its own is a group of one.

- A sync of any repo in a group exports the database once and files
  the beads of every repo in the group.
- `sync --all` pulls and syncs each group once.
- A group MUST NOT mix path entries and table entries.
- Table entries in a group MUST use the same `field`.
- Two table entries in a group MUST NOT share a value or name the same
  checkout.

A group that breaks a rule is neither pulled nor synced, and `sync`
exits non-zero. Other groups still sync.

A checkout not in `repos` whose database a group uses syncs as that
group. One whose database no group uses syncs alone, as a path entry
would, with a warning that its prefix is unchecked. While any table
entry is unresolvable, such a checkout syncs nothing, since its
database may be that entry's.

### Unclaimed beads

In a group of table entries, a bead is unclaimed when no entry claims
it, or when its entry's project is not in the vault.

- An unclaimed bead gets no note. A note it already has is left as it
  is. It is not marked removed, since the bead is still in beads.
- Once an entry claims it, the next sync creates its note.

Each such group has one index note:

```
<unfiled>/Unfiled beads - <prefix>.md
```

It lists each unclaimed bead that would otherwise get a note: id,
title, status and reason. Closed beads come after the rest. The reason
is that the field is absent, the value found (JSON for a value that is
not a string), or the missing project's name. callisto updates the
index on every sync, to an empty list when nothing is unclaimed. It
never deletes it. Like a bead's note, the index carries
`generatedBy: callisto`, and hand edits are lost.

callisto never creates the unfiled folder. If the folder is missing, or
the index path holds a file callisto did not write, no index is
written. A group of path entries has no unclaimed beads and no index.

A table entry is unresolvable when callisto cannot place it in a group
or resolve it: its checkout is missing, `bd context` fails, the
database's prefix cannot be read, or a defaulted `value` has no usable
`origin`. While any table entry is unresolvable, every group of table
entries:

- still creates and updates notes for beads a resolved entry claims;
- does not rewrite its index, since a bead that looks unclaimed may be
  the unresolvable entry's;
- does not mark notes removed.

Each of these is a standing condition, logged as described in
[Errors](#errors). `sync` exits non-zero for an unresolvable entry, a
broken group rule, a missing unfiled folder, an index path callisto did
not write, or an unlisted checkout it skipped. Unclaimed beads and a
missing project alone do not fail it.

## Commands

The global flag `--vault <dir>` overrides the config file's vault.

### `callisto sync [<repo>...]`

Syncs the given repo checkouts, by default the current directory's
repo. A worktree or jj workspace resolves to its main checkout. Prints
one line per repo:

```
ganymede: 2 created, 5 updated, 0 removed, 40 unchanged
```

Warnings and standing conditions go to stderr. Exits non-zero if any
repo failed, or on a condition listed under
[Unclaimed beads](#unclaimed-beads).

### `callisto sync --all`

Runs `bd dolt pull` once per beads database the configured repos use,
then syncs them. Repos that share a database sync together, once,
under the first such repo listed that exists. A database whose pull
fails is logged and skipped; the others still sync. This is what the
scheduled job runs.

`callisto sync` and the hook treat a shared database the same way: a
repo in a group syncs the whole group. See
[Shared databases](#shared-databases).

### `callisto hook`

A Claude Code and Codex `Stop` hook. It reads the hook payload on stdin
and syncs the repo its `cwd` is in, with that repo's group, so notes
catch up as soon as an agent finishes a turn. It does not pull.

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
    field = "metadata.repo";
    repos = [
      "~/Workspaces/callisto"
      "~/Workspaces/ganymede"
      {
        path = "~/work/api";
        value = "github.com/example/api";
        project = "API";
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
| `services.callisto.repos` | `[ ]` | Written to `config.toml`. Each is a path or a table entry with `path`, `value`, `field` and `project`. Each path MUST be absolute or start with `~/`. |
| `services.callisto.field` | `null` | Written to `config.toml`. |
| `services.callisto.unfiled` | `null` | Written to `config.toml`. Unset, callisto uses `Inbox`. |
| `services.callisto.path` | profile `bin`, then system dirs | `PATH` for the job. |
| `services.callisto.startInterval` | `900` | Seconds between launchd runs (macOS). |
| `services.callisto.onCalendar` | `"*:0/15"` | systemd timer schedule (Linux). |

Set `repos` through the module. Once `vault` or `repos` is set,
home-manager owns `config.toml`. `sync --all` fails without repos or
a vault, so every switch warns while `repos` is empty or `vault` is
unset. `field` and `unfiled` alone do not make home-manager own the
file; a switch warns that they have no effect.

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
- When a table entry omits `value`, the job's `PATH` MUST also reach
  `git`, which callisto runs to read the checkout's `origin`. The
  default `path` reaches it only if your profile or system provides
  it.
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
`vault`, `render`, `write`, `conditions`, or `panic`. The log is created
`0600`, since entries carry paths and `bd` output. It is kept under
1 MiB by dropping its older half.

Standing conditions are problems that persist across runs: unclaimed
beads, a missing project, a missing unfiled folder, an index path
callisto did not write, an unresolvable entry, a broken group rule, an
unlisted checkout skipped. `sync` prints them on every run. The log
gets them once when they appear or change, and once when they clear.
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
