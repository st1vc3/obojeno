{ lib, rustPlatform }:

let
  cargoToml = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = "obojeno";
  inherit (cargoToml.workspace.package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "--package" "obojeno" ];

  meta = {
    description = "Wallpaper-driven theming for macOS";
    homepage = "https://github.com/st1vc3/obojeno";
    license = lib.licenses.mit;
    platforms = lib.platforms.darwin;
    mainProgram = "obojeno";
  };
}
