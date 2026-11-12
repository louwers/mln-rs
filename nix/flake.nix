# The dev shell of the justfile's recipes on NixOS: the C++ repo's #linux dev
# shell, plus what the port needs on top of it. The justfile's `_dev-shell`
# copies it to .nix-shell, next to the C++ repo's flake in .nix-shell/cpp.
{
  inputs.cpp.url = "path:./cpp";

  outputs =
    { cpp, ... }:
    {
      devShells = builtins.mapAttrs (
        system: shells:
        let
          pkgs = cpp.inputs.nixpkgs.legacyPackages.${system};
        in
        {
          linux = shells.linux.overrideAttrs (old: {
            # winit (examples/desktop) loads the Wayland and X11 libraries at
            # run time, and mln_gl's headless backend loads EGL (libglvnd,
            # which finds the system's Mesa).
            LD_LIBRARY_PATH = "${old.LD_LIBRARY_PATH}:${
              pkgs.lib.makeLibraryPath [
                pkgs.libglvnd
                pkgs.libxkbcommon
                pkgs.wayland
                pkgs.libx11
                pkgs.libxcursor
                pkgs.libxi
                pkgs.libxrandr
              ]
            }";
          });
        }
      ) cpp.devShells;
    };
}
