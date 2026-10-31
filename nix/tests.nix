# Evaluation tests for the home-manager module. Every listed system is
# evaluated on any host; only `rendered` builds anything.
{
  self,
  home-manager,
  nixpkgs,
  systems,
}:
let
  inherit (nixpkgs) lib;

  isDarwin = lib.hasSuffix "darwin";

  hmConfig =
    {
      system,
      settings,
      extra ? { },
      pkgs ? nixpkgs.legacyPackages.${system},
    }:
    (home-manager.lib.homeManagerConfiguration {
      inherit pkgs;
      modules = [
        self.homeManagerModules.default
        {
          home.username = "alice";
          home.homeDirectory = if isDarwin system then "/Users/alice" else "/home/alice";
          home.stateVersion = "26.05";
          manual.manpages.enable = false;
          services.callisto = settings;
        }
        extra
      ];
    }).config;

  agent = cfg: cfg.launchd.agents.callisto;
  service = cfg: cfg.systemd.user.services.callisto;
  timer = cfg: cfg.systemd.user.timers.callisto;

  failed = cfg: map (a: a.message) (builtins.filter (a: !a.assertion) cfg.assertions);

  warned = cfg: builtins.filter (lib.hasPrefix "services.callisto") cfg.warnings;

  defPath =
    cfg:
    "${cfg.home.profileDirectory}/bin:/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:/usr/bin:/bin:/usr/sbin:/sbin";

  platformTests =
    system:
    let
      darwin = isDarwin system;
      home = if darwin then "/Users/alice" else "/home/alice";
      pkg = self.packages.${system}.default;

      with' =
        settings: extra:
        hmConfig {
          inherit system settings extra;
        };
      # A vault and a repo keep the warnings out of every other test.
      enabled =
        settings:
        with' (
          {
            enable = true;
            vault = "~/org";
            repos = [ "/srv/b" ];
          }
          // settings
        ) { };
      on = enabled { };
      bare = with' { enable = true; } { };
      off = with' { } { };
      moved =
        with'
          {
            enable = true;
            repos = [ "/srv/b" ];
          }
          {
            xdg.configHome = "/c";
            xdg.stateHome = "/s";
          };

      # launchd takes an attrset, systemd a list of K=V lines.
      jobEnv =
        cfg:
        if darwin then
          (agent cfg).config.EnvironmentVariables
        else
          lib.listToAttrs (
            map (
              line:
              let
                kv = builtins.match "([^=]*)=(.*)" line;
              in
              lib.nameValuePair (builtins.elemAt kv 0) (builtins.elemAt kv 1)
            ) (service cfg).Service.Environment
          );

      # home-manager throws on a failed assertion as soon as config is
      # read, so only the failure itself is observable.
      rejected = settings: !(builtins.tryEval (enabled settings).home.packages).success;

      common = {
        test_installs_package = {
          expr = builtins.elem pkg on.home.packages;
          expected = true;
        };

        test_disabled_is_inert = {
          expr = [
            (off.launchd.agents ? callisto)
            (off.systemd.user.services ? callisto)
            (off.systemd.user.timers ? callisto)
            (builtins.elem pkg off.home.packages)
          ];
          expected = [
            false
            false
            false
            false
          ];
        };

        test_job_env = {
          expr = jobEnv on;
          expected = {
            PATH = defPath on;
            XDG_CONFIG_HOME = "${home}/.config";
            XDG_STATE_HOME = "${home}/.local/state";
          };
        };

        # The hook runs with the user's XDG dirs; the job MUST share its
        # lock and error log.
        test_job_follows_moved_xdg_dirs = {
          expr = jobEnv moved;
          expected = {
            PATH = defPath moved;
            XDG_CONFIG_HOME = "/c";
            XDG_STATE_HOME = "/s";
          };
        };

        test_path_override = {
          expr =
            (jobEnv (enabled {
              path = [ "/x/bin" ];
            })).PATH;
          expected = "/x/bin";
        };

        test_no_config_without_settings = {
          expr = bare.xdg.configFile ? "callisto/config.toml";
          expected = false;
        };

        test_config_with_settings =
          let
            configured = enabled {
              vault = "~/org";
              repos = [
                "~/Workspaces/a"
                "/srv/b"
              ];
            };
          in
          {
            expr = {
              file = configured.xdg.configFile ? "callisto/config.toml";
              failed = failed configured;
            };
            expected = {
              file = true;
              failed = [ ];
            };
          };

        test_warns_without_repos = {
          expr = map (cfg: builtins.length (warned cfg)) [
            bare
            (enabled {
              vault = "~/org";
              repos = [ ];
            })
          ];
          expected = [
            1
            1
          ];
        };

        test_warns_without_vault = {
          expr = map (cfg: builtins.length (warned cfg)) [
            (with' {
              enable = true;
              repos = [ "/srv/b" ];
            } { })
          ];
          expected = [ 1 ];
        };

        test_no_warning_when_repos_reach_the_job = {
          expr = map (cfg: warned cfg) [
            on
            (with' { enable = true; } {
              xdg.configFile."callisto/config.toml".text = ''repos = ["/srv/b"]'';
            })
            off
          ];
          expected = [
            [ ]
            [ ]
            [ ]
          ];
        };

        test_relative_repo_fails_assertion = {
          expr = rejected { repos = [ "Workspaces/a" ]; };
          expected = true;
        };

        test_relative_vault_fails_assertion = {
          expr = rejected { vault = "org"; };
          expected = true;
        };
      };

      darwinTests = {
        test_agent = {
          expr = {
            inherit (agent on) enable;
            inherit ((agent on).config)
              ProgramArguments
              StartInterval
              RunAtLoad
              ProcessType
              StandardOutPath
              StandardErrorPath
              ;
          };
          expected = {
            enable = true;
            ProgramArguments = [
              "${pkg}/bin/callisto"
              "sync"
              "--all"
            ];
            StartInterval = 900;
            RunAtLoad = true;
            ProcessType = "Background";
            StandardOutPath = null;
            StandardErrorPath = null;
          };
        };

        test_no_systemd = {
          expr = [
            (on.systemd.user.services ? callisto)
            (on.systemd.user.timers ? callisto)
          ];
          expected = [
            false
            false
          ];
        };

        test_start_interval_override = {
          expr =
            (agent (enabled {
              startInterval = 300;
            })).config.StartInterval;
          expected = 300;
        };
      };

      linuxTests = {
        test_service = {
          expr = {
            inherit ((service on).Unit) Description;
            inherit ((service on).Service) Type ExecStart;
          };
          expected = {
            Description = "Mirror beads into the Obsidian vault";
            Type = "oneshot";
            ExecStart = [ "${pkg}/bin/callisto sync --all" ];
          };
        };

        test_timer = {
          expr = {
            inherit ((timer on).Unit) Description;
            inherit ((timer on).Timer) OnCalendar Persistent;
            inherit ((timer on).Install) WantedBy;
          };
          expected = {
            Description = "Mirror beads into the Obsidian vault on a schedule";
            OnCalendar = "*:0/15";
            Persistent = true;
            WantedBy = [ "timers.target" ];
          };
        };

        test_no_agent = {
          expr = on.launchd.agents ? callisto;
          expected = false;
        };

        test_on_calendar_override = {
          expr =
            (timer (enabled {
              onCalendar = "hourly";
            })).Timer.OnCalendar;
          expected = "hourly";
        };
      };
    in
    lib.mapAttrs' (name: lib.nameValuePair "${name} (${system})") (
      common // (if darwin then darwinTests else linuxTests)
    );

  # Each system's units as home-manager renders them, written by the
  # host's builders, so any host can check every platform's output.
  rendered =
    host:
    let
      hostPkgs = nixpkgs.legacyPackages.${host};
      render =
        system:
        let
          cfg = hmConfig {
            inherit system;
            pkgs = nixpkgs.legacyPackages.${system}.extend (
              _: _: { inherit (hostPkgs) runCommand writeText writeTextFile; }
            );
            settings = {
              enable = true;
              repos = [ "/srv/b" ];
              package = self.packages.${host}.default;
            };
          };
          # home-manager links the plists into the generation only here.
          cmds = cfg.home.extraBuilderCommands;
          agents = builtins.elemAt (builtins.match ''(.*${"\n"})?ln -s "([^"]*)" [$]out/LaunchAgents${"\n"}.*'' cmds) 1;
          unit = name: cfg.xdg.configFile."systemd/user/${name}".source;
        in
        if isDarwin system then
          {
            # Only the agents drv: another Darwin line here would make a
            # Linux check need a Darwin builder.
            plist = builtins.appendContext "${agents}/org.nix-community.home.callisto.plist" (
              lib.filterAttrs (drv: _: lib.hasSuffix "-home-manager-agents.drv" drv) (builtins.getContext cmds)
            );
          }
        else
          {
            service = unit "callisto.service";
            timer = unit "callisto.timer";
          };
      all = map render systems;
    in
    {
      # Systems of one platform render the same files.
      plists = lib.unique (map (r: r.plist) (builtins.filter (r: r ? plist) all));
      units = lib.unique (builtins.filter (r: r ? service) all);
    };

  failures = lib.runTests (lib.mergeAttrsList (map platformTests systems));
in
{
  inherit failures hmConfig rendered;
}
