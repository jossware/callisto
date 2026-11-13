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
      # A vault and a database keep the warnings out of every other test.
      enabled =
        settings:
        with' (
          {
            enable = true;
            vault = "~/org";
            databases = [ { beads_dir = "/srv/b/.beads"; } ];
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
            databases = [ { beads_dir = "/srv/b/.beads"; } ];
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
              databases = [
                { beads_dir = "~/Workspaces/a/.beads"; }
                { beads_dir = "/srv/b/.beads"; }
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

        test_warns_without_databases = {
          expr = map (cfg: builtins.length (warned cfg)) [
            bare
            (enabled {
              vault = "~/org";
              databases = [ ];
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
              databases = [ { beads_dir = "/srv/b/.beads"; } ];
            } { })
          ];
          expected = [ 1 ];
        };

        test_no_warning_when_databases_reach_the_job = {
          expr = map (cfg: warned cfg) [
            on
            (with' { enable = true; } {
              xdg.configFile."callisto/config.toml".text = ''
                [[databases]]
                beads_dir = "/srv/b/.beads"
              '';
            })
            off
          ];
          expected = [
            [ ]
            [ ]
            [ ]
          ];
        };

        test_relative_beads_dir_rejected = {
          expr = map rejected [
            { databases = [ { beads_dir = "Workspaces/a/.beads"; } ]; }
            { databases = [ { beads_dir = "~user/.beads"; } ]; }
            { databases = [ { beads_dir = "~/a/.beads"; } ]; }
          ];
          expected = [
            true
            true
            false
          ];
        };

        test_old_options_removed = {
          expr = map rejected [
            { repos = [ "/srv/b" ]; }
            { field = "metadata.repo"; }
          ];
          expected = [
            true
            true
          ];
        };

        test_relative_vault_fails_assertion = {
          expr = rejected { vault = "org"; };
          expected = true;
        };

        test_databases_accepted =
          let
            configured = enabled {
              unfiled = "Inbox/Beads";
              databases = [
                { beads_dir = "/srv/b/.beads"; }
                {
                  beads_dir = "~/.dotfiles/.beads";
                  project = "dotfiles";
                }
                {
                  beads_dir = "~/work/tasksdb/.beads";
                  split.by = "metadata.repo";
                  split.projects."github.com/o/api" = "API";
                }
                {
                  beads_dir = "/srv/q/.beads";
                  split.by = "x";
                }
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

        test_project_and_split_rejected = {
          expr = rejected {
            databases = [
              {
                beads_dir = "/srv/p/.beads";
                project = "P";
                split.by = "metadata.repo";
              }
            ];
          };
          expected = true;
        };

        test_bad_split_by_fails_assertion = {
          expr = map (by: rejected {
            databases = [
              {
                beads_dir = "/p/.beads";
                split.by = by;
              }
            ];
          }) [
            "metadata."
            ".repo"
            "a..b"
            "metadata.repo"
          ];
          expected = [
            true
            true
            true
            false
          ];
        };

        test_bad_unfiled_fails_assertion = {
          expr = map (u: rejected { unfiled = u; }) [
            "/abs"
            "Inbox/"
            "a/../b"
            "./a"
            ""
            "a//b"
          ];
          expected = [
            true
            true
            true
            true
            true
            true
          ];
        };

        # The loader reads a leading ~ as an ordinary directory name.
        test_tilde_unfiled_is_relative = {
          expr = rejected { unfiled = "~/x"; };
          expected = false;
        };

        test_warns_on_unused_unfiled = {
          expr = warned (with' {
            enable = true;
            unfiled = "U";
          } { xdg.configFile."callisto/config.toml".text = ""; });
          expected = [
            "services.callisto.unfiled is set, but neither vault nor databases is, so config.toml is not generated and it has no effect."
          ];
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
              _: _: {
                inherit (hostPkgs)
                  linkFarm
                  runCommand
                  writeText
                  writeTextFile
                  ;
              }
            );
            settings = {
              enable = true;
              databases = [ { beads_dir = "/srv/b/.beads"; } ];
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
