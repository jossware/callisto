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

  nonEmptyStr = lib.types.strMatching ".+";

  entryPath = e: if lib.isString e then e else e.path;

  # Mirrors the loader: every dot-separated segment is non-empty.
  fieldAssertion = option: f: {
    assertion = !(builtins.elem "" (lib.splitString "." f));
    message = "services.callisto.${option}: ${f} MUST be a dotted path with no empty segments";
  };

  unfiledAssertion = u: {
    assertion =
      !(lib.hasPrefix "/" u)
      && !(builtins.any (s: builtins.elem s [ "" "." ".." ]) (lib.splitString "/" u));
    message = "services.callisto.unfiled: ${u} MUST be a relative path inside the vault, with no empty, `.` or `..` segments";
  };

  repoAssertions =
    e:
    let
      p = entryPath e;
    in
    [ (entryAssertion "repos" p) ]
    ++ lib.optionals (lib.isAttrs e) (
      [
        {
          assertion = e.field != null || cfg.field != null;
          message = "services.callisto.repos: ${p} MUST set field, or set services.callisto.field";
        }
      ]
      ++ lib.optional (e.field != null) (fieldAssertion "repos" e.field)
    );

  renderRepo =
    e:
    if lib.isString e then
      e
    else
      lib.filterAttrs (_: v: v != null) { inherit (e) path value field project; };

  generates = cfg.vault != null || cfg.repos != [ ];

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
        config.toml or `--vault`. With neither this nor `repos` set,
        config.toml is left alone.
      '';
    };

    repos = lib.mkOption {
      type = lib.types.listOf (
        lib.types.either lib.types.str (
          lib.types.submodule {
            options = {
              path = lib.mkOption {
                type = lib.types.str;
                description = "Repo checkout. MUST be absolute or start with `~/`.";
              };
              value = lib.mkOption {
                type = lib.types.nullOr (lib.types.either nonEmptyStr (lib.types.nonEmptyListOf nonEmptyStr));
                default = null;
                description = ''
                  Value or values matched against the field on each bead.
                  callisto defaults it to the checkout's `origin`
                  normalised to `host/path`.
                '';
              };
              field = lib.mkOption {
                type = lib.types.nullOr lib.types.str;
                default = null;
                description = ''
                  Dotted path of the bead field to match. Overrides
                  `services.callisto.field`. A table entry MUST have one
                  of the two.
                '';
              };
              project = lib.mkOption {
                type = lib.types.nullOr nonEmptyStr;
                default = null;
                description = ''
                  Project name for the repo. callisto defaults it to the
                  checkout directory's name.
                '';
              };
            };
          }
        )
      );
      default = [ ];
      example = lib.literalExpression ''
        [
          "~/Workspaces/callisto"
          { path = "~/Workspaces/other"; value = "github.com/o/other"; project = "Other"; }
        ]
      '';
      description = ''
        Repo checkouts written to config.toml, synced by
        `callisto sync --all`. Each is a path or a table with a `path`.
        Every path MUST be absolute or start with `~/`.
      '';
    };

    field = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "metadata.repo";
      description = ''
        Dotted path of the bead field that table entries match, unless
        they set their own. Unset, callisto applies no default.
      '';
    };

    unfiled = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "Inbox/Beads";
      description = ''
        Vault-relative directory for beads that match no repo. callisto
        defaults it to `Inbox`.
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
          ++ lib.concatMap repoAssertions cfg.repos
          ++ lib.optional (cfg.field != null) (fieldAssertion "field" cfg.field)
          ++ lib.optional (cfg.unfiled != null) (unfiledAssertion cfg.unfiled);

        # sync --all fails without a vault or with an empty repo list, so
        # the job would log an error every run. A config.toml
        # home-manager does not manage may still supply either, hence
        # warnings and not assertions.
        warnings =
          lib.optional ((cfg.field != null || cfg.unfiled != null) && !generates)
            "services.callisto: field and unfiled are set, but neither vault nor repos is, so config.toml is not generated and they have no effect."
          ++ (if cfg.repos != [ ] then
            lib.optional (cfg.vault == null)
              "services.callisto.vault is unset, so every scheduled `callisto sync --all` fails. Set services.callisto.vault."
          else if cfg.vault != null then
            [
              "services.callisto.repos is empty, so every scheduled `callisto sync --all` fails. Set services.callisto.repos."
            ]
          else
            lib.optional (!(config.xdg.configFile ? "callisto/config.toml"))
              "services.callisto: neither vault nor repos is set, so the scheduled `callisto sync --all` fails unless ${config.xdg.configHome}/callisto/config.toml sets vault and lists repos. Set services.callisto.vault and services.callisto.repos, or manage that file through xdg.configFile, to silence this.");

        xdg.configFile."callisto/config.toml" = lib.mkIf generates {
          source = (pkgs.formats.toml { }).generate "callisto-config.toml" (
            lib.filterAttrs (_: v: v != null) {
              inherit (cfg) vault field unfiled;
              repos = map renderRepo cfg.repos;
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
