{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.callisto;
  inherit (pkgs.stdenv.hostPlatform) isDarwin isLinux;

  # callisto resolves config entries without a working directory, so
  # only these two forms mean the same thing on every run.
  anchored = p: lib.hasPrefix "/" p || lib.hasPrefix "~/" p;
  entryAssertion = option: p: {
    assertion = anchored p;
    message = "services.callisto.${option}: ${p} MUST be absolute or start with ~/";
  };

  nonEmpty = lib.types.strMatching ".+";

  # Mirrors the loader: every dot-separated segment is non-empty.
  fieldAssertion = option: label: f: {
    assertion = !(builtins.elem "" (lib.splitString "." f));
    message = "services.callisto.${option}: ${label} ${f} MUST be a dotted path with no empty segments";
  };

  unfiledAssertion = u: {
    assertion =
      !(lib.hasPrefix "/" u)
      && !(builtins.any (s: builtins.elem s [ "" "." ".." ]) (lib.splitString "/" u));
    message = "services.callisto.unfiled: ${u} MUST be a relative path inside the vault, with no empty, `.` or `..` segments";
  };

  databaseAssertions =
    d:
    [
      (entryAssertion "databases" d.beads_dir)
      {
        assertion = d.project == null || d.split == null;
        message = "services.callisto.databases: ${d.beads_dir} MUST NOT set both project and split";
      }
    ]
    ++ lib.optional (d.split != null) (
      fieldAssertion "databases" "${d.beads_dir}: split.by" d.split.by
    );

  renderDatabase =
    d:
    lib.filterAttrs (_: v: v != null) {
      inherit (d) beads_dir project;
      split =
        if d.split == null then
          null
        else
          { inherit (d.split) by; }
          // lib.optionalAttrs (d.split.projects != { }) { inherit (d.split) projects; };
    };

  generates = cfg.vault != null || cfg.databases != [ ];

  exe = "${cfg.package}/bin/callisto";
  # The hook runs with the user's XDG dirs; launchd and systemd do not
  # pass them on, and a job on other dirs would take a different lock.
  environment = {
    PATH = lib.concatStringsSep ":" cfg.path;
    XDG_CONFIG_HOME = config.xdg.configHome;
    XDG_STATE_HOME = config.xdg.stateHome;
  };
  description = "Mirror beads into the Obsidian vault";
in
{
  options.services.callisto = {
    enable = lib.mkEnableOption "scheduled callisto sync --all";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "callisto.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The callisto package to install and schedule.";
    };

    vault = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "~/org";
      description = ''
        Vault root written to config.toml. callisto needs a vault from
        config.toml or `--vault`. With neither this nor `databases` set,
        config.toml is left alone.
      '';
    };

    databases = lib.mkOption {
      type = lib.types.listOf (
        lib.types.submodule {
          options = {
            beads_dir = lib.mkOption {
              type = lib.types.str;
              description = "Beads database directory. MUST be absolute or start with `~/`.";
            };
            project = lib.mkOption {
              type = lib.types.nullOr nonEmpty;
              default = null;
              description = ''
                Project every bead in the database goes to. callisto
                defaults it to the database's `custom.obsidian.project`
                setting, else the name of the directory that holds
                `beads_dir`. MUST NOT be set with `split`.
              '';
            };
            split = lib.mkOption {
              type = lib.types.nullOr (
                lib.types.submodule {
                  options = {
                    by = lib.mkOption {
                      type = lib.types.str;
                      description = "Dotted path of the bead field that names its project.";
                    };
                    projects = lib.mkOption {
                      type = lib.types.attrsOf nonEmpty;
                      default = { };
                      description = ''
                        Field value to project name. A value not listed
                        files under its last `/`-separated segment.
                      '';
                    };
                  };
                }
              );
              default = null;
              description = "Files each bead by a field, for a database many repos share.";
            };
          };
        }
      );
      default = [ ];
      example = lib.literalExpression ''
        [
          { beads_dir = "~/Workspaces/callisto/.beads"; }
          {
            beads_dir = "~/work/tasksdb/.beads";
            split.by = "metadata.repo";
            split.projects."github.com/example/api" = "API";
          }
        ]
      '';
      description = ''
        Beads databases written to config.toml, synced by
        `callisto sync --all`.
      '';
    };

    unfiled = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "Inbox/Beads";
      description = ''
        Vault-relative folder for each split database's index of
        unfiled beads, `Unfiled beads - <prefix>.md`. Unfiled beads get
        no note of their own. callisto never creates the folder, and a
        sync fails if it is missing. callisto defaults it to `Inbox`.
      '';
    };

    path = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [
        "${config.home.profileDirectory}/bin"
        "/run/current-system/sw/bin"
        "/nix/var/nix/profiles/default/bin"
        "/usr/bin"
        "/bin"
        "/usr/sbin"
        "/sbin"
      ];
      defaultText = lib.literalExpression ''
        [ "''${config.home.profileDirectory}/bin" "/run/current-system/sw/bin" "/nix/var/nix/profiles/default/bin" "/usr/bin" "/bin" "/usr/sbin" "/sbin" ]
      '';
      description = ''
        PATH for the scheduled job. It MUST reach `bd` and whatever
        `bd dolt pull` needs (git, ssh).
      '';
    };

    startInterval = lib.mkOption {
      type = lib.types.ints.positive;
      default = 900;
      description = "Seconds between runs of the launchd agent (macOS).";
    };

    onCalendar = lib.mkOption {
      type = lib.types.str;
      default = "*:0/15";
      description = "When the systemd timer fires (Linux). See systemd.time(7).";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        home.packages = [ cfg.package ];

        assertions =
          lib.optional (cfg.vault != null) (entryAssertion "vault" cfg.vault)
          ++ lib.concatMap databaseAssertions cfg.databases
          ++ lib.optional (cfg.unfiled != null) (unfiledAssertion cfg.unfiled);

        # sync --all fails without a vault or with an empty database list,
        # so the job would log an error every run. A config.toml
        # home-manager does not manage may still supply either, hence
        # warnings and not assertions.
        warnings =
          lib.optional (!generates && cfg.unfiled != null)
            "services.callisto.unfiled is set, but neither vault nor databases is, so config.toml is not generated and it has no effect."
          ++ (if cfg.databases != [ ] then
            lib.optional (cfg.vault == null)
              "services.callisto.vault is unset, so every scheduled `callisto sync --all` fails. Set services.callisto.vault."
          else if cfg.vault != null then
            [
              "services.callisto.databases is empty, so every scheduled `callisto sync --all` fails. Set services.callisto.databases."
            ]
          else
            lib.optional (!(config.xdg.configFile ? "callisto/config.toml"))
              "services.callisto: neither vault nor databases is set, so the scheduled `callisto sync --all` fails unless ${config.xdg.configHome}/callisto/config.toml sets vault and lists databases. Set services.callisto.vault and services.callisto.databases, or manage that file through xdg.configFile, to silence this.");

        xdg.configFile."callisto/config.toml" = lib.mkIf generates {
          source = (pkgs.formats.toml { }).generate "callisto-config.toml" (
            lib.filterAttrs (_: v: v != null) {
              inherit (cfg) vault unfiled;
              databases = map renderDatabase cfg.databases;
            }
          );
        };
      }

      (lib.mkIf isDarwin {
        launchd.agents.callisto = {
          enable = true;
          config = {
            ProgramArguments = [
              exe
              "sync"
              "--all"
            ];
            StartInterval = cfg.startInterval;
            RunAtLoad = true;
            ProcessType = "Background";
            EnvironmentVariables = environment;
          };
        };
      })

      (lib.mkIf isLinux {
        systemd.user.services.callisto = {
          Unit.Description = description;
          Service = {
            Type = "oneshot";
            ExecStart = "${exe} sync --all";
            Environment = lib.mapAttrsToList (k: v: "${k}=${v}") environment;
          };
        };

        systemd.user.timers.callisto = {
          Unit.Description = "${description} on a schedule";
          Timer = {
            OnCalendar = cfg.onCalendar;
            Persistent = true;
          };
          Install.WantedBy = [ "timers.target" ];
        };
      })
    ]
  );
}
