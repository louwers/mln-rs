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
            # glfw-sys's `wayland` feature (examples/glfw) asks pkg-config for
            # wayland-client.
            buildInputs = old.buildInputs ++ [ pkgs.wayland ];
            # rustc links with its own lld, past the Nix ld wrapper that would
            # add the GLFW the app links to its rpath.
            LD_LIBRARY_PATH = "${old.LD_LIBRARY_PATH}:${pkgs.lib.makeLibraryPath [ pkgs.glfw ]}";
          });
        }
      ) cpp.devShells;
    };
}
