#!/usr/bin/env bash
set -euo pipefail
[[ $(id -u) != 0 ]] || exit 2
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
export MIXRELAY_IPC_DIR="${MIXRELAY_IPC_DIR:-$XDG_RUNTIME_DIR/mixrelay}"
[[ "$MIXRELAY_IPC_DIR" == /* ]] || { echo 'MIXRELAY_IPC_DIR must be absolute' >&2; exit 2; }
engine_socket="$MIXRELAY_IPC_DIR/engine.sock"
systemctl --user status mixrelay-lab-engine --no-pager
journalctl --user -u mixrelay-lab-engine --no-pager -n 40
for route in health api/v1/status api/v1/graph api/v1/meters; do
  printf '\n%s\n' "$route"
  curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --fail --silent --show-error --max-time 5 "http://mixrelay-engine/$route"
done
wpctl status
