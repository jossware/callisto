{
  description = "callisto: mirror beads issues into an Obsidian vault";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      home-manager,
    }:
    let
      # nixpkgs no longer evaluates x86_64-darwin.
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      tests = import ./nix/tests.nix {
        inherit
          self
          home-manager
          nixpkgs
          systems
          ;
      };
    in
    {
      homeManagerModules = rec {
        callisto = import ./nix/hm-module.nix { inherit self; };
        default = callisto;
      };
    }
    // flake-utils.lib.eachSystem systems (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
        toml =
          settings:
          (tests.hmConfig {
            inherit system;
            settings = {
              enable = true;
            }
            // settings;
          }).xdg.configFile."callisto/config.toml".source;
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = manifest.name;
          inherit (manifest) version;
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          meta = {
            mainProgram = manifest.name;
            license = pkgs.lib.licenses.mit;
          };
        };

        # A sample user's units, rendered by home-manager for this host.
        packages.units =
          let
            hm = tests.hmConfig {
              inherit system;
              settings = {
                enable = true;
                databases = [ { beads_dir = "~/Workspaces/callisto/.beads"; } ];
              };
            };
            unit = name: hm.xdg.configFile."systemd/user/${name}".source;
          in
          pkgs.runCommand "callisto-units" { } (
            if pkgs.stdenv.hostPlatform.isDarwin then
              ''
                mkdir -p $out
                cp -L ${hm.home.activationPackage}/LaunchAgents/org.nix-community.home.callisto.plist $out/
              ''
            else
              ''
                mkdir -p $out
                cp -L ${unit "callisto.service"} $out/callisto.service
                cp -L ${unit "callisto.timer"} $out/callisto.timer
              ''
          );

        checks.hm-module =
          if tests.failures == [ ] then
            pkgs.runCommand "callisto-hm-module" { } "touch $out"
          else
            throw "hm-module tests failed: ${builtins.toJSON tests.failures}";

        checks.config-toml =
          pkgs.runCommand "callisto-config-toml"
            {
              nativeBuildInputs = [ pkgs.python3 ];
              full = toml {
                vault = "~/org";
                locations = [
                  "Work"
                  "Archive/Work"
                ];
                databases = [
                  { beads_dir = "~/Workspaces/a/.beads"; }
                  {
                    beads_dir = "/srv/b/.beads";
                    project = "B";
                  }
                  {
                    beads_dir = "/srv/c/.beads";
                    split.by = "metadata.repo";
                    split.projects."github.com/o/api" = "API";
                  }
                  {
                    beads_dir = "/srv/d/.beads";
                    split.by = "x";
                  }
                ];
              };
              databasesOnly = toml { databases = [ { beads_dir = "/srv/b/.beads"; } ]; };
            }
            ''
              python3 - <<'PY'
              import os, tomllib
              def load(var):
                  with open(os.environ[var], "rb") as f:
                      return tomllib.load(f)
              full = load("full")
              assert full == {
                  "vault": "~/org",
                  "locations": ["Work", "Archive/Work"],
                  "databases": [
                      {"beads_dir": "~/Workspaces/a/.beads"},
                      {"beads_dir": "/srv/b/.beads", "project": "B"},
                      {
                          "beads_dir": "/srv/c/.beads",
                          "split": {
                              "by": "metadata.repo",
                              "projects": {"github.com/o/api": "API"},
                          },
                      },
                      {"beads_dir": "/srv/d/.beads", "split": {"by": "x"}},
                  ],
              }, full
              databases_only = load("databasesOnly")
              assert databases_only == {"databases": [{"beads_dir": "/srv/b/.beads"}]}, databases_only
              PY
              touch $out
            '';

        # Runs here because the test needs the module's generated files.
        checks.config-loader = self.packages.${system}.default.overrideAttrs {
          pname = "callisto-config-loader";
          CALLISTO_HM_CONFIG_DIR = pkgs.linkFarm "callisto-hm-configs" [
            {
              name = "single.toml";
              path = toml {
                vault = "~/org";
                databases = [
                  { beads_dir = "~/Workspaces/callisto/.beads"; }
                  {
                    beads_dir = "~/.dotfiles/.beads";
                    project = "dotfiles";
                  }
                ];
              };
            }
            {
              name = "split.toml";
              path = toml {
                vault = "~/org";
                unfiled = "Inbox/Beads";
                locations = [
                  "Work"
                  "Archive/Work"
                ];
                databases = [
                  {
                    beads_dir = "/srv/tasksdb/.beads";
                    split.by = "metadata.repo";
                    split.projects = {
                      "github.com/o/api" = "API";
                      "github.com/o/web" = "Web";
                    };
                  }
                  {
                    beads_dir = "/srv/other/.beads";
                    split.by = "x.y";
                  }
                ];
              };
            }
          ];
          cargoTestFlags = [
            "--test"
            "hm_config"
          ];
          checkFlags = [ "--ignored" ];
        };

        checks.units =
          let
            inherit (tests.rendered system) plists units;
          in
          pkgs.runCommand "callisto-units-check" { nativeBuildInputs = [ pkgs.python3 ]; } ''
            python3 - ${pkgs.lib.escapeShellArgs plists} <<'PY'
            import plistlib, sys
            assert sys.argv[1:], "no plist rendered"
            for path in sys.argv[1:]:
                with open(path, "rb") as f:
                    agent = plistlib.load(f)
                assert agent["Label"] == "org.nix-community.home.callisto", agent
                assert agent["StartInterval"] == 900, agent
                assert agent["ProgramArguments"][-1].endswith("/bin/callisto sync --all"), agent
            PY
            ${pkgs.lib.concatMapStrings (u: ''
              grep -E '^ExecStart=.*/bin/callisto sync --all$' ${u.service}
              grep -Fx 'OnCalendar=*:0/15' ${u.timer}
            '') units}
            touch $out
          '';

        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.default ];
          packages = with pkgs; [
            cargo-insta
            clippy
            rust-analyzer
            rustfmt
          ];
        };
      }
    );
}
