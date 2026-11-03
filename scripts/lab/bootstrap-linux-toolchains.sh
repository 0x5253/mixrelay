#!/usr/bin/env bash
set -euo pipefail

# Run as the existing non-root guest account. No passwords, shell-profile edits,
# host mounts, or global npm packages are required. TLS and pinned hashes remain
# enabled; failed downloads are never executed.
[[ $(id -u) != 0 ]] || { echo "Run as the guest user, not root" >&2; exit 2; }
[[ $(uname -s) == Linux ]] || exit 2
case "$(uname -m)" in
  aarch64)
    node_arch=arm64
    rust_arch=aarch64
    node_hash=01443c1e1a29e531ccad5a46fefa6df490d2189c49f7955904aecdbb0fe86fdc
    rust_hash=e3853c5a252fca15252d07cb23a1bdd9377a8c6f3efa01531109281ae47f841c
    ;;
  x86_64)
    node_arch=x64
    rust_arch=x86_64
    node_hash=14b342e71204f811bde6153be8e04b62aef63c236fef92b55f9c83154b409647
    rust_hash=20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c
    ;;
  *) echo "Unsupported architecture" >&2; exit 2 ;;
esac
toolchain_root="$HOME/.local/share/mixrelay-lab"
mkdir -p "$toolchain_root"
download_dir=$(mktemp -d "$toolchain_root/download.XXXXXXXX")
node_name="node-v24.19.0-linux-$node_arch"
curl --fail --location --retry 3 --proto '=https' --tlsv1.2 \
  "https://nodejs.org/dist/v24.19.0/$node_name.tar.xz" -o "$download_dir/node.tar.xz"
printf '%s  %s\n' "$node_hash" "$download_dir/node.tar.xz" | sha256sum --check --strict
if [[ ! -d "$toolchain_root/$node_name" ]]; then
  tar -xJf "$download_dir/node.tar.xz" -C "$toolchain_root"
fi
export PATH="$toolchain_root/$node_name/bin:$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
[[ $(node --version) == v24.19.0 ]]
npm install --global --prefix "$HOME/.local" pnpm@11.9.0
curl --fail --location --retry 3 --proto '=https' --tlsv1.2 \
  "https://static.rust-lang.org/rustup/archive/1.28.2/$rust_arch-unknown-linux-gnu/rustup-init" \
  -o "$download_dir/rustup-init"
printf '%s  %s\n' "$rust_hash" "$download_dir/rustup-init" | sha256sum --check --strict
chmod u+x "$download_dir/rustup-init"
"$download_dir/rustup-init" -y --no-modify-path --profile minimal --default-toolchain 1.96.0
rustc --version --verbose
pnpm --version
# Downloads are retained in this private task directory for provenance/retry.
