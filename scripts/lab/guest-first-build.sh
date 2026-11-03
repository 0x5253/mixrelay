#!/usr/bin/env bash
set -euo pipefail
[[ $(id -u) != 0 ]] || exit 2
archive=${1:?Expected transferred source archive}
case "$(uname -m)" in
  aarch64) node_arch=arm64 ;;
  x86_64) node_arch=x64 ;;
  *) exit 2 ;;
esac
export PATH="$HOME/.local/share/mixrelay-lab/node-v24.19.0-linux-$node_arch/bin:$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS=2 CI=true
workspace="$HOME/mixrelay"
mkdir "$workspace" # Never replace an existing guest checkout.
tar -xzf "$archive" -C "$workspace"
cd "$workspace"
uname -a
cat /etc/os-release
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
# A real logged-in desktop session is required for these audio checks.
systemctl --user is-active pipewire wireplumber
wpctl status
pnpm install --frozen-lockfile
node scripts/lab/native-check.mjs test
