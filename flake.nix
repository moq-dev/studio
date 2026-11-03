{
  description = "MoQ Studio: a native streaming studio over Media over QUIC";

  # moq's binary cache also holds the toolchain and tools this shell shares
  # with moq's. `fallback` builds whatever is missing instead of failing.
  nixConfig = {
    extra-substituters = [ "https://nix.moq.dev" ];
    extra-trusted-public-keys = [
      "nix.moq.dev-1:BZ0DtCypDFv1ESX3hxuB42yc0vitSA1YHojSmVwB5bk="
    ];
    fallback = true;
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # The quest CLI, which also serves the quest guide and skills the stubs in
    # .claude/skills call. Bump the rev to upgrade them.
    quest = {
      url = "github:moq-dev/quest/4be350816f2a5c1157f799773ee1594f199fd9a7";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      quest,
      ...
    }:
    flake-utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ]
      (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          inherit (pkgs) lib;
          isLinux = pkgs.stdenv.hostPlatform.isLinux;

          # Pinned to the toolchain moq builds with (rust-toolchain.toml says
          # why), so the submodule's crates compile here as they do there.
          rust-toolchain = pkgs.rust-bin.stable."1.98.1".minimal.override {
            # `minimal` rather than `default`, which adds hundreds of MB of
            # offline HTML docs to a closure every CI job downloads.
            extensions = [
              "rustfmt"
              "clippy"
              "rust-src"
              "rust-analyzer"
            ];
          };

          quest-cli = quest.packages.${system}.default;

          # Libraries the window opens at runtime rather than links: winit
          # dlopens X11, Wayland, and xkbcommon, and wgpu dlopens the Vulkan
          # loader. Outside NixOS a Nix-built binary cannot find the host's.
          runtimeLibs = lib.optionals isLinux (
            with pkgs;
            [
              vulkan-loader
              libxkbcommon
              wayland
              libx11
              libxcursor
              libxi
              libxrandr
            ]
          );

          # Where the shell's libasound looks for PCM plugins. ALSA's default
          # device is a plugin on any desktop running a sound server, and the
          # shell's alsa-lib only searches its own store path, so without this
          # the default device fails to open. ALSA_PLUGIN_DIR takes one
          # directory, hence the join.
          alsaPlugins = pkgs.symlinkJoin {
            name = "studio-alsa-plugins";
            paths = [
              pkgs.alsa-plugins
              pkgs.pipewire
            ];
          };
        in
        {
          packages.quest = quest-cli;

          devShells.default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                rust-toolchain
                just
                git
                jq
                cmake
                nasm
                pkg-config
                # libclang for the bindgen builds (pipewire, VAAPI).
                rustPlatform.bindgenHook
                cargo-nextest
                cargo-deny
                cargo-sort
                cargo-shear
                quest-cli

                # `just check` lints.
                shellcheck
                shfmt
                actionlint
                taplo
                nixfmt
              ]
              ++ lib.optionals isLinux [
                # cpal's alsa-sys links libasound for the `capture` feature.
                alsa-lib
                # moq-video's and moq-audio's `pipewire` features link libpipewire.
                pipewire
              ];

            shellHook = ''
              # `nix develop` points TMPDIR at a fresh nix-shell.* dir that it
              # never removes. Drop it and keep the caller's TMPDIR.
              if [[ ''${NIX_BUILD_TOP-} == */nix-shell.* && -d $NIX_BUILD_TOP ]]; then
                rm -rf "$NIX_BUILD_TOP"
                export TMPDIR=''${NIX_BUILD_TOP%/*}
                unset NIX_BUILD_TOP TMP TEMP TEMPDIR
              fi

              # The pinned toolchain shadows the Cargo shim `mbx setup`
              # installs. Put it back in front, as moq's shell does, so a bare
              # `cargo` here reaches the same wrapper it reaches outside.
              status=$(mbx setup --status 2>/dev/null || true)
              shim=$(printf '%s\n' "$status" | sed -n '1s/.*: //p')
              if [ -x "$shim" ]; then
                export PATH="$(dirname "$shim"):$PATH"
              fi

              # Nested shells prepend this shell's store paths again; keep each
              # directory's first entry so PATH stays under the kernel's limit.
              PATH=$(printf '%s' "$PATH" | awk -v RS=: '!seen[$0]++ { printf "%s%s", sep, $0; sep = ":" }')
            '';

            env = lib.optionalAttrs isLinux {
              ALSA_PLUGIN_DIR = "${alsaPlugins}/lib/alsa-lib";
              LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
            };
          };

          formatter = pkgs.nixfmt-tree;
        }
      );
}
