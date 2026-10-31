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
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "~/Workspaces/callisto" ];
      description = ''
        Repo checkouts written to config.toml, synced by
        `callisto sync --all`. Each MUST be absolute or start with `~/`.
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
          ++ map (entryAssertion "repos") cfg.repos;

        # sync --all fails without a vault or with an empty repo list, so
        # the job would log an error every run. A config.toml
        # home-manager does not manage may still supply either, hence
        # warnings and not assertions.
        warnings =
          if cfg.repos != [ ] then
            lib.optional (cfg.vault == null)
              "services.callisto.vault is unset, so every scheduled `callisto sync --all` fails. Set services.callisto.vault."
          else if cfg.vault != null then
            [
              "services.callisto.repos is empty, so every scheduled `callisto sync --all` fails. Set services.callisto.repos."
            ]
          else
            lib.optional (!(config.xdg.configFile ? "callisto/config.toml"))
              "services.callisto: neither vault nor repos is set, so the scheduled `callisto sync --all` fails unless ${config.xdg.configHome}/callisto/config.toml sets vault and lists repos. Set services.callisto.vault and services.callisto.repos, or manage that file through xdg.configFile, to silence this.";

        xdg.configFile."callisto/config.toml" = lib.mkIf (cfg.vault != null || cfg.repos != [ ]) {
          source = (pkgs.formats.toml { }).generate "callisto-config.toml" (
            lib.filterAttrs (_: v: v != null) { inherit (cfg) vault repos; }
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
