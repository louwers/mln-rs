{
  description = "The C toolchain and the libraries that mln-rs builds, tests and runs with on Linux";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      inherit (nixpkgs) lib;

      forAllSystems =
        f: lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        # `nix develop` enters it on any Linux with Nix, and the justfile's
        # recipes run in it on NixOS, which has no system-wide libraries for
        # a build or a dlopen() to find. Rust and the other tools come from
        # mise (mise.toml); this adds the C toolchain, for the crates that
        # compile C, and the libraries.
        default = pkgs.mkShell {
          # tools/c-abi-test builds maplibre-native-ffi's ABI suite with
          # CMake, against the Vulkan headers and loader, and for its OpenGL
          # preset EGL and OpenGL ES (libglvnd).
          packages = [ pkgs.cmake ];

          # sqlite-wasm-rs compiles SQLite for wasm32 (`just check-wasm`)
          # through the cc crate, which reads these before CC (gcc here).
          # Unwrapped clang, since the Nix wrapper adds the host's flags.
          CC_wasm32_unknown_unknown = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";
          AR_wasm32_unknown_unknown = "${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-ar";

          buildInputs = [
            pkgs.vulkan-headers
            pkgs.vulkan-loader
            pkgs.libglvnd
          ];

          # Loaded with dlopen() at run time: the Vulkan loader by wgpu, EGL
          # by mln_gl's headless backend (libglvnd, which finds the system's
          # Mesa), the Wayland and X11 libraries by winit (examples/desktop),
          # and libwayland by the ABI suite's Wayland cases.
          LD_LIBRARY_PATH = lib.makeLibraryPath [
            pkgs.vulkan-loader
            pkgs.libglvnd
            pkgs.libxkbcommon
            pkgs.wayland
            pkgs.libx11
            pkgs.libxcursor
            pkgs.libxi
            pkgs.libxrandr
          ];

          # wgpu validates a debug build's Vulkan calls when the loader finds
          # the validation layer.
          VK_ADD_LAYER_PATH = "${pkgs.vulkan-validation-layers}/share/vulkan/explicit_layer.d";
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
