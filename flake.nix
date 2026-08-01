{
  description = "pgfs — PostgreSQL-backed FUSE filesystem";

  inputs = {
    # Pin the entire userland/toolchain.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system:
          f nixpkgs.legacyPackages.${system}
        );
    in
    {
      devShells = forAllSystems (pkgs:
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              # Rust
              rustc
              cargo
              rustfmt
              clippy

              # PostgreSQL
              postgresql

              # FUSE
              fuse3

              # Native build dependencies
              pkg-config

              # Useful development tools
              gdb
              lldb
              git
              jq
            ];

            # fuser's build script uses pkg-config to find FUSE.
            PKG_CONFIG_PATH = pkgs.lib.makeSearchPath "lib/pkgconfig" [
              pkgs.fuse3
            ];

            shellHook = ''
              echo "pgfs development environment"
              echo
              echo "Rust:       $(rustc --version)"
              echo "Cargo:      $(cargo --version)"
              echo "Postgres:   $(postgres --version)"
              echo "FUSE:       $(pkg-config --modversion fuse3)"
              echo

              export PGFS_ROOT="$PWD"
              export PGFS_TESTDATA="$PWD/testdata"
              export PGFS_PGDATA="$PWD/testdata/pgdata"
              export PGFS_SOCKDIR="$PWD/testdata"
            '';
          };
        });
    };
}
