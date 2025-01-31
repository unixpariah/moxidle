{
  pkg-config,
  lua5_4,
  libpulseaudio,
  lib,
  rustPlatform,
}:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = "moxidle";
  version = "${cargoToml.package.version}";
  cargoLock.lockFile = ../Cargo.lock;
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.intersection (lib.fileset.fromSource (lib.sources.cleanSource ../.)) (
      lib.fileset.unions [
        ../src
        ../Cargo.toml
        ../Cargo.lock
      ]
    );
  };

  nativeBuildInputs = [
    pkg-config
  ];

  buildInputs = [
    lua5_4
    libpulseaudio
  ];

  configurePhase = ''
    export PKG_CONFIG_PATH=${lua5_4}/lib/pkgconfig:${libpulseaudio}/lib/pkgconfig
  '';

  meta = with lib; {
    description = "Idle daemon with conditional timeouts and build in audio inhibitor";
    mainProgram = "moxidle";
    homepage = "https://github.com/unixpariah/moxidle";
    license = licenses.gpl3;
    maintainers = with maintainers; [ unixpariah ];
    platforms = platforms.unix;
  };
}
