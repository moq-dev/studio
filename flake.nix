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
      url = "github:moq-dev/quest/6a18b8a815c2d15d37d7433a619f961ec5a07165";
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

          # CEF for browser sources (`cef` feature), at the version the `cef`
          # crate in Cargo.lock binds: bump them together. cef-dll-sys builds
          # its wrapper from this instead of downloading CEF, and a dev build
          # loads libcef.so from it. Patched like nixpkgs' cef-binary, whose
          # install step expects GL libraries this release no longer ships.
          cefArchive = {
            x86_64-linux = {
              name = "cef_binary_156.0.15+gcf671c0+chromium-156.0.8078.80_linux64_minimal";
              hash = "sha256-aHP5dbsEcxZoDqGDmYDtW9TDPIzUzkV2HJQhGFZgU4g=";
              sha1 = "09235427222e026a4f34399742df27c5d98a4f8a";
            };
            aarch64-linux = {
              name = "cef_binary_156.0.15+gcf671c0+chromium-156.0.8078.80_linuxarm64_minimal";
              hash = "sha256-gGZjqeyiqPe+GFSAzn53nFhktc3VK9d0iQU72xkz/rg=";
              sha1 = "85602db8e813ad824b45ae54218476aa3d91d561";
            };
          };
          cef = lib.optionalAttrs isLinux (
            let
              archive = cefArchive.${system};
            in
            pkgs.stdenv.mkDerivation {
              pname = "cef";
              version = "156.0.15";
              src = pkgs.fetchurl {
                url = "https://cef-builds.spotifycdn.com/${lib.escapeURL archive.name}.tar.bz2";
                inherit (archive) hash;
              };
              nativeBuildInputs = [ pkgs.autoPatchelfHook ];
              buildInputs = with pkgs; [
                alsa-lib
                at-spi2-atk
                at-spi2-core
                atk
                cairo
                cups
                dbus
                expat
                glib
                gtk3
                libdrm
                libgbm
                libx11
                libxcb
                libxcomposite
                libxdamage
                libxext
                libxfixes
                libxkbcommon
                libxrandr
                libxshmfence
                nspr
                nss
                pango
                stdenv.cc.cc
              ];
              # libEGL, which libcef.so opens at run time rather than links:
              # without it the GPU process cannot start, and pages paint in
              # memory. runtimeDependencies would reach only executables.
              appendRunpaths = [ "${lib.getLib pkgs.libGL}/lib" ];
              dontStrip = true;
              installPhase = ''
                runHook preInstall
                mkdir $out
                cp -r Release/. Resources/. CMakeLists.txt cmake include libcef_dll $out/
                # cef-dll-sys copies every file here beside the binaries, where
                # the studio loads the Vulkan loader first and ANGLE looks for
                # nowhere else. CEF's own finds no NixOS driver; this one does.
                ln -sf ${lib.getLib pkgs.vulkan-loader}/lib/libvulkan.so.1 $out/libvulkan.so.1
                # What cef-dll-sys checks the version against.
                echo '{"type":"minimal","name":"${archive.name}.tar.bz2","sha1":"${archive.sha1}"}' > $out/archive.json
                runHook postInstall
              '';
            }
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
                # cef-dll-sys builds CEF's C++ wrapper with Ninja.
                ninja
                pkg-config
                # libclang for the bindgen builds (pipewire, VAAPI).
                rustPlatform.bindgenHook
                cargo-nextest
                cargo-deny
                cargo-sort
                cargo-shear
                quest-cli

                # The join page: install, lint, type-check, test, and build.
                bun

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
              CEF_PATH = "${cef}";
              LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
            };
          };

          formatter = pkgs.nixfmt-tree;
        }
      );
}
