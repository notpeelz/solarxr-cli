{
  pkgs,
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "solarxr-cli";
  version = "0.1.0";

  src = ../.;
  cargoLock.lockFile = ../Cargo.lock;

  buildInputs = with pkgs; [
    openxr-loader
  ];

  nativeBuildInputs = with pkgs; [
    makeWrapper
  ] ++ [
    (pkgs.writeShellScriptBin "cargo" ''
      if [[ "$#" -ge 1 && "$1" == "build" ]]; then
        shift 1
        exec ${pkgs.cargo}/bin/cargo make install build -- "$@"
      else
        exec ${pkgs.cargo}/bin/cargo "$@"
      fi
    '')
  ];

  installPhase = ''
    runHook preInstall
    mkdir -p "$out"
    cp -r build/. "$out"/.
    runHook postInstall
  '';

  postFixup = ''
    wrapProgram "$out/bin/solarxr-input" \
      --prefix XDG_CONFIG_DIRS : "$out/etc/xdg"
  '';

  meta = {
    description = "solarxr-cli";
    homepage = "https://github.com/notpeelz/solarxr-cli";
    license = lib.licenses.gpl3;
    maintainers = with lib.maintainers; [ different-name ];
    mainProgram = "solarxr-cli";
  };
}
