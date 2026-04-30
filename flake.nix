{
  description = "OmnySSH — TUI SSH dashboard & server manager";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

        omnyssh = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          version = cargoToml.package.version;

          src = self;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = [ pkgs.pkg-config pkgs.installShellFiles ];

          postInstall = ''
            installManPage doc/omny.1
          '';

          meta = with pkgs.lib; {
            description = cargoToml.package.description;
            homepage = cargoToml.package.repository;
            license = licenses.asl20;
            mainProgram = "omny";
          };
        };
      in
      {
        packages = {
          default = omnyssh;
          omnyssh = omnyssh;
        };

        apps.default = {
          type = "app";
          program = "${omnyssh}/bin/omny";
          meta = omnyssh.meta;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ omnyssh ];
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
          ];

          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
}
