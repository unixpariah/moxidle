{
  pkg-config,
  lua5_4,
  libpulseaudio,
  lib,
  rustPlatform,
  installShellFiles,
  scdoc,
}:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = "moxidle";
  version = "${cargoToml.package.version}";
  cargoLock.lockFile = ../Cargo.lock;
  src = lib.fileset.toSource {
    root = ./..;
    fileset = lib.fileset.intersection (lib.fileset.fromSource (lib.sources.cleanSource ./..)) (
      lib.fileset.unions [
        ../src
        ../Cargo.toml
        ../Cargo.lock
        ../doc
        ../completions
      ]
    );
  };

  nativeBuildInputs = [
    pkg-config
    scdoc
  ];

  buildInputs = [
    lua5_4
    libpulseaudio
    installShellFiles
  ];

  postInstall = ''
    for f in doc/*.scd; do
      local page="doc/$(basename "$f" .scd)"
      scdoc < "$f" > "$page"
      installManPage "$page"
    done

    installShellCompletion --cmd moxidle \
      --bash completions/moxidle.bash \
      --fish completions/moxidle.fish \
      --zsh completions/_moxidle
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
