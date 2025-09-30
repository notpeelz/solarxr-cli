{
  description = "solarxr-cli";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    systems.url = "github:nix-systems/default";
    self.submodules = true;
  };

  outputs =
    {
      self,
      nixpkgs,
      systems,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      eachSystem = lib.genAttrs (import systems);
    in
    {
      packages =
        assert lib.assertMsg (self.submodules or false) ''
          This flake's repository contains submodules, but they were not fetched.
          The GitHub fetcher cannot fetch submodules (see https://github.com/NixOS/nix/issues/13571).
          Use the Git fetcher and append '?submodules=1' to the URI, e.g.:
            nix build 'git+https://github.com/notpeelz/solarxr-cli?submodules=1'
        '';
        eachSystem (system: {
          default = self.packages.${system}.solarxr-cli;
          solarxr-cli = nixpkgs.legacyPackages.${system}.callPackage ./nix { };
        });

      formatter = eachSystem (system: nixpkgs.legacyPackages.${system}.nixfmt-rfc-style);
    };
}
