{
  pkg-config,
  lua5_4,
  libpulseaudio,
  lib,
  rustPlatform,
  withDbus ? true,
  withSystemd ? true,
  withUpower ? true,
  withPulse ? true,
}:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  enabledFeatures = lib.concatMapStringsSep "," (feature: feature) (
    lib.optional withDbus "dbus"
    ++ lib.optional withSystemd "systemd"
    ++ lib.optional withUpower "upower"
    ++ lib.optional withPulse "audio"
  );
  pkgConfigPathStr = lib.concatStringsSep "" (
    lib.optional withPulse ":${libpulseaudio}/lib/pkgconfig"
  );
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

  nativeBuildInputs = [ pkg-config ];

  buildInputs = [
    lua5_4
  ] ++ lib.optional withPulse libpulseaudio;

  cargoBuildFlags =
    [
      "--no-default-features"
    ]
    ++ lib.optionals (enabledFeatures != "") [
      "--features=${enabledFeatures}"
    ];

  configurePhase = ''
    export PKG_CONFIG_PATH=${lua5_4}/lib/pkgconfig${pkgConfigPathStr}
  '';

  meta = with lib; {
    description = "Idle daemon with conditional timeouts and built-in audio inhibitor";
    mainProgram = "moxidle";
    homepage = "https://github.com/unixpariah/moxidle";
    license = licenses.gpl3;
    maintainers = with maintainers; [ unixpariah ];
    platforms = platforms.unix;
  };
}
