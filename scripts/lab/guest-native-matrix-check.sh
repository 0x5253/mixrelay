#!/usr/bin/env bash
set -euo pipefail
[[ $(uname -s) == Linux && $(id -u) != 0 ]] || exit 2
engine=${1:?Expected engine binary}
node=${2:?Expected Node binary}
probe=${3:?Expected matrix probe}
[[ "$engine" == /* && -x "$engine" && "$node" == /* && -x "$node" ]] || exit 2
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
export MIXRELAY_IPC_DIR="${MIXRELAY_IPC_DIR:-$XDG_RUNTIME_DIR/mixrelay}"
[[ "$MIXRELAY_IPC_DIR" == /* ]] || { echo 'MIXRELAY_IPC_DIR must be absolute' >&2; exit 2; }
engine_socket="$MIXRELAY_IPC_DIR/engine.sock"
if curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --silent --max-time 2 http://mixrelay-engine/health >/dev/null; then
  echo "Refusing to replace an existing engine" >&2; exit 2
fi
systemd-run --user --unit=mixrelay-lab-matrix --collect --setenv="MIXRELAY_IPC_DIR=$MIXRELAY_IPC_DIR" "$engine"
trap 'systemctl --user stop mixrelay-lab-matrix' EXIT
ready=false
for ((n=0; n<100; n++)); do
  if curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --silent --fail --max-time 1 http://mixrelay-engine/health >/dev/null; then ready=true; break; fi
  sleep .1
done
[[ "$ready" == true ]] || exit 1
MIXRELAY_LAB_OWNED_ENGINE_PID=$(systemctl --user show mixrelay-lab-matrix --property=MainPID --value)
export MIXRELAY_LAB_OWNED_ENGINE_PID
[[ "$MIXRELAY_LAB_OWNED_ENGINE_PID" =~ ^[1-9][0-9]*$ ]] || exit 1
sha256sum "$engine" "$probe"
"$node" "$probe" --run
systemctl --user stop mixrelay-lab-matrix
trap - EXIT
# Query by exact process identity, not names that another application may own.
"$node" --input-type=module -e '
  import {execFileSync} from "node:child_process";
  import assert from "node:assert/strict";
  import {pathToFileURL} from "node:url";
  const {parsePwDump,nodesOwnedByPid}=await import(new URL("./pipewire-cli.mjs",pathToFileURL(process.argv[1])));
  const pid=process.env.MIXRELAY_LAB_OWNED_ENGINE_PID;
  const objects=parsePwDump(execFileSync("pw-dump", {encoding:"utf8"}));
  assert.equal(nodesOwnedByPid(objects,pid).length,0);
  assert.equal(objects.some(o=>String(o.info?.props?.["application.process.id"])===pid),false);
  console.log("PASS: shutdown removed all PipeWire objects owned by engine PID "+pid);
' "$probe"
