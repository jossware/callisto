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
                repos = [ "~/Workspaces/callisto" ];
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
          let
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
          pkgs.runCommand "callisto-config-toml"
            {
              nativeBuildInputs = [ pkgs.python3 ];
              full = toml {
                vault = "~/org";
                repos = [
                  "~/Workspaces/a"
                  "/srv/b"
                ];
              };
              reposOnly = toml { repos = [ "/srv/b" ]; };
            }
            ''
              python3 - <<'PY'
              import os, tomllib
              def load(var):
                  with open(os.environ[var], "rb") as f:
                      return tomllib.load(f)
              full = load("full")
              assert full == {"vault": "~/org", "repos": ["~/Workspaces/a", "/srv/b"]}, full
              repos_only = load("reposOnly")
              assert repos_only == {"repos": ["/srv/b"]}, repos_only
              PY
              touch $out
            '';

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
