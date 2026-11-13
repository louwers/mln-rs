# Sourced by the tasks in mise-tasks/ that build or run native code. On NixOS,
# that code builds and runs inside flake.nix's dev shell, which has the C
# toolchain and the libraries it loads (the Vulkan loader, Wayland, X11, ...);
# elsewhere (macOS, CI) the system's libraries are the environment.
#
# `dev_shell <command>...` runs the command in the dev shell on NixOS and as
# is elsewhere. The shell comes from a copy of the flake in .nix-shell:
# `nix develop` on the repository copies its tracked files into the store
# again after every change to one.

if [[ -e /etc/NIXOS ]]; then
    mkdir -p .nix-shell
    for file in flake.nix flake.lock; do
        cmp -s "$file" ".nix-shell/$file" || cp "$file" ".nix-shell/$file"
    done
    # Absolute, since a task may call dev_shell from another directory.
    dev_shell_flake="path:$PWD/.nix-shell"
    dev_shell() {
        nix develop "$dev_shell_flake" --command "$@"
    }
else
    dev_shell() {
        "$@"
    }
fi
