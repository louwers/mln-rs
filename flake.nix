{
  description = "The environment that mln-rs builds, tests and runs in on NixOS";

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
        # The shell that the justfile's recipes run in. Rust and the other
        # tools come from mise (mise.toml); this adds the C toolchain, for the
        # crates that compile C, and the libraries that NixOS keeps out of
        # the places a build or a dlopen() would search.
        default = pkgs.mkShell {
          # tools/c-abi-test builds maplibre-native-ffi's ABI suite with
          # CMake, against the Vulkan headers and loader.
          packages = [ pkgs.cmake ];

          buildInputs = [
            pkgs.vulkan-headers
            pkgs.vulkan-loader
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
