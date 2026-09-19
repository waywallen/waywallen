{
  description = "Development environment for waywallen";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    lito = {
      url = "github:litocpp/lito";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
      lito,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rustfmt"
            "clippy"
          ];
        };

        qt6Env = pkgs.symlinkJoin {
          name = "qt6-unified-env";
          paths = [
            pkgs.qt6.qtbase
            pkgs.qt6.qtdeclarative
            pkgs.qt6.qtwebsockets
            pkgs.qt6.qtgrpc
            pkgs.qt6.qttools
            pkgs.protobuf
          ];
        };

        litoPkg = lito.packages.${system}.default;

        litoWrapped = pkgs.writeShellScriptBin "lito" ''
          exec ${litoPkg}/bin/lito \
            --use-env-flags \
            --config "tools.cmake.search-path=[\"${qt6Env}\"]" \
            "$@"
        '';

        rustLinker = pkgs.writeShellScript "rust-linker-wrapper" ''
          exec ${pkgs.llvmPackages.clang-unwrapped}/bin/clang \
            -L${pkgs.glibc}/lib \
            -L${pkgs.gcc-unwrapped.lib}/lib \
            -Wl,--dynamic-linker=${pkgs.glibc}/lib/ld-linux-x86-64.so.2 \
            -Wl,-rpath,${pkgs.glibc}/lib \
            -Wl,-rpath,${pkgs.gcc-unwrapped.lib}/lib \
            -fuse-ld=${pkgs.llvmPackages.lld}/bin/ld.lld \
            "$@"
        '';
      in
      {
        packages.lito = litoWrapped;

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            ninja
            protobuf
            rustToolchain
            glslang
            litoWrapped
          ];

          buildInputs = with pkgs; [
            libpulseaudio
            wayland
            wayland-protocols
            ffmpeg
            libgbm
            libva
            freetype
            expat
            dav1d
            lz4
            vulkan-loader
            vulkan-headers
            libGL
            libxkbcommon
            qt6Env
          ];

          CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = "${rustLinker}";

          CC = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          LITO_ALLOWED_ROOTS = "/nix/store";

          C_INCLUDE_PATH = "${pkgs.vulkan-headers}/include";
          CPLUS_INCLUDE_PATH = "${pkgs.vulkan-headers}/include";
          CFLAGS = "-I${pkgs.vulkan-headers}/include";
          CXXFLAGS = "-I${pkgs.vulkan-headers}/include";

          CMAKE_PREFIX_PATH = "${qt6Env};${pkgs.libxkbcommon}";
          CMAKE_MODULE_PATH = "${qt6Env}/lib/cmake/Qt6;${qt6Env}/lib/cmake/Qt6/3rdparty/kwin";
          Qt6_DIR = "${qt6Env}/lib/cmake/Qt6";
          Qt6Protobuf_DIR = "${qt6Env}/lib/cmake/Qt6Protobuf";
          Qt6ProtobufTools_DIR = "${qt6Env}/lib/cmake/Qt6ProtobufTools";

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              vulkan-loader
              libGL
              wayland
              libgbm
              ffmpeg
              libpulseaudio
              libva
              gcc-unwrapped.lib
              qt6Env
            ]
          );
        };
      }
    );
}
