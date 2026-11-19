{
  description = "obojeno - wallpaper-driven theming for macOS";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        obojeno = pkgs.callPackage ./nix/package.nix { };
        default = obojeno;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.obojeno ];
          packages = with pkgs; [ clippy rustfmt rust-analyzer cargo-insta ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
