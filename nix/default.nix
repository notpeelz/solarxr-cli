{
  self,
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "solarxr-cli";
  version = "0.1.0";

  src = self;

  cargoLock.lockFile = ../Cargo.lock;

  meta = {
    description = "solarxr-cli";
    homepage = "https://github.com/notpeelz/solarxr-cli";
    license = lib.licenses.gpl3;
    maintainers = with lib.maintainers; [ different-name ];
    mainProgram = "solarxr-cli";
  };
}
