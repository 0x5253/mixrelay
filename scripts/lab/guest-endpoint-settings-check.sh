#!/usr/bin/env bash
set -euo pipefail
[[ $(uname -s) == Linux && $(id -u) != 0 ]] || exit 2
workspace=${1:?Expected existing guest workspace}
[[ "$workspace" == /* && -f "$workspace/Cargo.toml" ]] || exit 2
export PATH="$HOME/.cargo/bin:$PATH" CARGO_BUILD_JOBS=2
cd "$workspace"
uname -a
cargo build --locked -p mixrelay-engine-service
cargo test --locked -p mixrelay-engine-service --lib --bins
cargo clippy --locked -p mixrelay-engine-service --lib --bins --tests -- -D warnings
