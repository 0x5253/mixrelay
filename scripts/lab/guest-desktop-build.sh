#!/usr/bin/env bash
set -euo pipefail
[[ $(id -u) != 0 ]] || exit 2
export PATH="$HOME/.cargo/bin:$PATH" CARGO_BUILD_JOBS=2
cd "$HOME/mixrelay"
# Plain cargo build otherwise expects the development URL; this embeds the
# already-built frontend so the guest desktop does not depend on the Mac's Vite.
cargo build --locked -p mixrelay-desktop --features tauri/custom-protocol
