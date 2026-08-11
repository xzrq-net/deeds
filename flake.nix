{
  description = "deeds: repo-local task graph for coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    crane.url = "github:ipetkov/crane";
  };

  outputs = {
    self,
    nixpkgs,
    crane,
    ...
  }: let
    pkgs = import nixpkgs {
      system = "x86_64-linux";
      config.allowAliases = false;
    };

    craneLib = crane.mkLib pkgs;

    deeds = craneLib.buildPackage {
      src = pkgs.lib.cleanSource ./.;
      postInstall = ''
        mkdir -p "$out/share/zsh/site-functions"
        COMPLETE=zsh "$out/bin/deeds" > "$out/share/zsh/site-functions/_deeds"
        # fpath autoload must also complete on its first invocation.
        echo '_clap_dynamic_completer_deeds "$@"' >> "$out/share/zsh/site-functions/_deeds"
      '';
    };
  in {
    packages.x86_64-linux = {
      default = deeds;
      inherit deeds;
    };

    checks.x86_64-linux = {
      inherit deeds;
    };

    devShells.x86_64-linux.default = pkgs.mkShell {
      RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";

      packages = [
        pkgs.cargo
        pkgs.clippy
        pkgs.rust-analyzer
        pkgs.rustc
        pkgs.rustfmt
      ];
    };
  };
}
