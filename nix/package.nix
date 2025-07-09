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

  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      let
        relPath = lib.removePrefix (toString ../. + "/") (toString path);
      in
      lib.any (p: lib.hasPrefix p relPath) [
        "src"
        "Cargo.toml"
        "Cargo.lock"
        "doc"
        "contrib"
        "completions"
      ];
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

    # This is for integration with moxctl
    ln -s $out/bin/moxidle $out/bin/moxidlectl
  '';

  meta = with lib; {
    description = "Idle daemon with conditional listeners and built-in audio inhibitor";
    mainProgram = "moxidle";
    homepage = "https://github.com/unixpariah/moxidle";
    license = licenses.mit;
    maintainers = with maintainers; [ unixpariah ];
    platforms = platforms.unix;
  };
}
