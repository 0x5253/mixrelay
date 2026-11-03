#!/usr/bin/env bash
set -euo pipefail
[[ $(uname -s) == Linux && $(id -u) == 1000 ]] || exit 2
workspace=${1:?Expected frozen source directory}
target=${2:?Expected exact owned Cargo target directory}
manifest=${3:?Expected frozen source manifest}
manifest_sha=${4:?Expected source manifest SHA-256}
ppm=${5:?Expected deliberate sender ppm}
seconds=${6:-90}
disable=${7:-false}
engine=${8:?Expected current release engine binary}
engine_sha=${9:?Expected release engine SHA-256}
node_binary=/home/lab/.local/share/mixrelay-lab/node-v24.19.0-linux-arm64/bin/node
fixture="$workspace/native/linux/pipewire/tests"
analysis="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/analyze-clock-drift.mjs"
sender_source="$(dirname -- "$analysis")/MRDriftSender.c"
quantum=${MIXRELAY_LAB_QUANTUM:-128}
[[ "$quantum" == 128 || "$quantum" == 1024 ]] || exit 2
rt_module=''
if [[ ${MIXRELAY_LAB_RT_ENABLED:-false} == true ]]; then
    rt_module='{ name = libpipewire-module-rt args = { rt.prio = 20 rt.time.soft = 200000 rt.time.hard = 2000000 } }'
fi
[[ "$workspace" == /home/lab/.lab/* && "$target" == /home/lab/.lab/* ]] || exit 2
[[ -f "$workspace/Cargo.toml" && -f "$sender_source" && -f "$fixture/MRDeviceFixture.c" && -f "$manifest" && -f "$analysis" && -x "$node_binary" ]] || exit 2
[[ "$workspace" == "$(realpath -e -- "$workspace")" && "$target" == "$(realpath -e -- "$target")" && "$engine" == "$(realpath -e -- "$engine")" ]] || exit 2
[[ $(stat -c %u "$workspace") == 1000 && $(stat -c %u "$target") == 1000 ]] || exit 2
[[ "$manifest_sha" =~ ^[0-9a-f]{64}$ ]] || exit 2
[[ $(sha256sum "$manifest" | cut -d ' ' -f 1) == "$manifest_sha" ]] || exit 2
[[ "$engine_sha" =~ ^[0-9a-f]{64}$ && $(stat -c %u "$engine") == 1000 ]] || exit 2
[[ $(sha256sum "$engine" | cut -d ' ' -f 1) == "$engine_sha" ]] || exit 2
python3 - "$workspace" "$manifest" <<'PY'
import hashlib
import json
import pathlib
import stat
import sys

root = pathlib.Path(sys.argv[1])
manifest = pathlib.Path(sys.argv[2])
record = json.loads(manifest.read_text())
archive = pathlib.Path(str(manifest).removesuffix('.manifest.json'))
if hashlib.sha256(archive.read_bytes()).hexdigest() != record['sha256']:
    raise SystemExit('frozen source archive hash changed')
clock_paths = {
    'apps/service/src/linux.rs',
    'apps/service/src/linux_clock.rs',
    'apps/service/src/linux_devices.rs',
    'native/linux/pipewire/src/stream.rs',
}
clock_prefixes = ('native/pcm-ring/', 'native/linux/pipewire/')
selected = [member for member in record['files'] if member['path'] in clock_paths or member['path'].startswith(clock_prefixes)]
if len(selected) < 20:
    raise SystemExit('frozen manifest lacks the Linux clock boundary')
for member in selected:
    relative = pathlib.PurePosixPath(member['path'])
    if relative.is_absolute() or '..' in relative.parts:
        raise SystemExit(f'invalid manifest path: {relative}')
    path = root.joinpath(*relative.parts)
    for part in [path, *path.parents]:
        if part == root.parent:
            break
        if part.is_symlink():
            raise SystemExit(f'symlink in frozen source: {relative}')
    if not stat.S_ISREG(path.stat().st_mode):
        raise SystemExit(f'non-regular frozen source: {relative}')
    if hashlib.sha256(path.read_bytes()).hexdigest() != member['sha256']:
        raise SystemExit(f'frozen clock-boundary mismatch: {relative}')
print(f'frozen_clock_boundary_verified={len(selected)} manifest={manifest}')
PY
[[ "$ppm" == 2000 && "$seconds" == 60 && "$disable" == false && "$quantum" == 128 ]] || {
    printf '%s\n' 'Current-engine drift receipt is pinned to +2000 ppm, 60 s, adaptive RTP SRC, 128-frame quantum' >&2
    exit 2
}
[[ ${MIXRELAY_LAB_RT_ENABLED:-false} == true ]] || { echo 'Current-engine receipt requires the approved RT module request' >&2; exit 2; }
for dependency in ss cc pkg-config pipewire wireplumber dbus-daemon busctl prlimit pw-dump curl jq; do
    command -v "$dependency" >/dev/null || { printf 'Missing required fixture tool: %s\n' "$dependency" >&2; exit 2; }
done
port=49178
[[ -z $(ss -H -lun "sport = :$port") ]] || { echo 'Owned RTP port is occupied'; exit 2; }
mkdir -p "$target/native-tests"
lab=$(mktemp -d "$target/native-tests/drift-lab-XXXXXX")
printf 'isolated_lab=%s engine_sha256=%s ppm=%s seconds=%s resample.disable=%s quantum=%s\n' "$lab" "$engine_sha" "$ppm" "$seconds" "$disable" "$quantum"
printf 'node_version=%s\n' "$("$node_binary" --version)"
socket_bytes=$(printf '%s' "$lab/runtime/pipewire-0-manager" | wc -c)
(( socket_bytes < 108 )) || { printf 'Private lab socket path is too long; use a shorter workspace mount\n' >&2; exit 2; }
mkdir -p "$lab/runtime" "$lab/config" "$lab/state" "$lab/data" "$lab/engine-config"
chmod 700 "$lab/runtime"
bash "$workspace/native/linux/pipewire/tests/configure-application-policy.sh" \
    "$workspace/native/linux/pipewire/tests" "$lab"
sed -e "s/@PORT@/$port/" -e "s/@DISABLE@/$disable/" -e "s/@QUANTUM@/$quantum/g" -e "s/@RT_MODULE@/$rt_module/" "$fixture/drift-lab-pipewire.conf" > "$lab/pipewire.conf"
export XDG_RUNTIME_DIR="$lab/runtime" XDG_CONFIG_HOME="$lab/config" XDG_STATE_HOME="$lab/state" XDG_DATA_HOME="$lab/data"
export MIXRELAY_IPC_DIR="$lab/ipc"
engine_socket="$MIXRELAY_IPC_DIR/engine.sock"
[[ ! -e "$engine_socket" && ! -L "$engine_socket" ]] || { echo 'Engine control socket is occupied'; exit 2; }
export WIREPLUMBER_CONFIG_DIR="$lab/config/wireplumber"
unset PIPEWIRE_REMOTE PIPEWIRE_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS
cc -O2 -Wall -Wextra -Werror "$sender_source" -lm -o "$lab/drift-sender"
read -r -a pipewire_flags <<< "$(pkg-config --cflags --libs libpipewire-0.3)"
cc -O2 -Wall -Wextra -Werror "$fixture/MRDeviceFixture.c" \
    "${pipewire_flags[@]}" -lm -o "$lab/device-fixture"
cc -O2 -Wall -Wextra -Werror "$fixture/MRDeviceFixtureGapTest.c" \
    "${pipewire_flags[@]}" -lm -o "$lab/device-fixture-gap-test"
"$lab/device-fixture-gap-test" >"$lab/device-fixture-gap-test.log"
printf '%s  %s\n' "$engine_sha" "$engine" >"$lab/engine.sha256"
sha256sum "$analysis" "$sender_source" "$fixture/MRDeviceFixture.c" "${BASH_SOURCE[0]}" >"$lab/fixture.sha256"
pipewire_pid='' policy_pid='' bus_pid='' sender_pid='' fixture_pid='' engine_pid=''
declare -A owned_start=() owned_exe=()
proc_identity() {
    local record
    local -a fields
    IFS= read -r record <"/proc/$1/stat" || return 1
    record=${record##*) }
    read -r -a fields <<<"$record"
    [[ ${fields[19]:-} =~ ^[0-9]+$ ]] || return 1
    [[ ${fields[1]:-} =~ ^[0-9]+$ ]] || return 1
    printf '%s %s %s\n' "${fields[0]}" "${fields[1]}" "${fields[19]}"
}
record_owned() {
    local owned_pid=$1 expected=$2 identity state parent start executable
    [[ "$owned_pid" =~ ^[1-9][0-9]*$ ]] || return 1
    expected=$(realpath -e -- "$expected") || return 1
    identity=$(proc_identity "$owned_pid") || return 1
    read -r state parent start <<<"$identity"
    [[ "$parent" == "$$" ]] || return 1
    owned_start[$owned_pid]=$start
    for _ in {1..20}; do
        identity=$(proc_identity "$owned_pid") || return 1
        read -r state parent start <<<"$identity"
        [[ "$parent" == "$$" && "$start" == "${owned_start[$owned_pid]}" ]] || return 1
        executable=$(readlink -e -- "/proc/$owned_pid/exe") || return 1
        if [[ "$executable" == "$expected" ]]; then
            owned_exe[$owned_pid]=$executable
            return 0
        fi
        sleep 0.01
    done
    return 1
}
owned_state() {
    local owned_pid=$1 identity state parent start executable mismatch=0
    [[ -e "/proc/$owned_pid" ]] || { printf 'gone\n'; return; }
    if ! identity=$(proc_identity "$owned_pid"); then
        [[ -e "/proc/$owned_pid" ]] || { printf 'gone\n'; return; }
        identity=$(proc_identity "$owned_pid") || {
            [[ -e "/proc/$owned_pid" ]] && printf 'unverified\n' || printf 'gone\n'
            return
        }
    fi
    read -r state parent start <<<"$identity"
    [[ "$parent" == "$$" && "$start" == "${owned_start[$owned_pid]:-}" ]] || { printf 'foreign\n'; return; }
    [[ "$state" != Z && "$state" != X ]] || { printf 'stopped\n'; return; }
    if executable=$(readlink -e -- "/proc/$owned_pid/exe"); then
        if [[ "$executable" == "${owned_exe[$owned_pid]:-}" ]]; then printf 'live\n'; return; fi
        mismatch=1
    fi
    [[ -e "/proc/$owned_pid" ]] || { printf 'gone\n'; return; }
    identity=$(proc_identity "$owned_pid") || {
        [[ -e "/proc/$owned_pid" ]] && printf 'unverified\n' || printf 'gone\n'
        return
    }
    read -r state parent start <<<"$identity"
    [[ "$parent" == "$$" && "$start" == "${owned_start[$owned_pid]:-}" ]] || { printf 'foreign\n'; return; }
    [[ "$state" != Z && "$state" != X ]] || { printf 'stopped\n'; return; }
    if (( mismatch )); then printf 'foreign\n'; else printf 'unverified\n'; fi
}
stop_owned() {
    local owned_pid=$1
    local exit_code=0 state
    [[ "$owned_pid" =~ ^[1-9][0-9]*$ ]] || return 0
    [[ -n ${owned_start[$owned_pid]:-} && -n ${owned_exe[$owned_pid]:-} ]] || {
        printf 'owned_process_identity_missing pid=%s\n' "$owned_pid" >&2
        return 1
    }
    state=$(owned_state "$owned_pid")
    if [[ "$state" == live ]]; then kill -TERM "$owned_pid" 2>/dev/null || true; fi
    for _ in {1..20}; do
        state=$(owned_state "$owned_pid")
        [[ "$state" == live ]] || break
        sleep 0.05
    done
    state=$(owned_state "$owned_pid")
    if [[ "$state" == live ]]; then
        kill -KILL "$owned_pid" 2>/dev/null || true
        for _ in {1..20}; do
            state=$(owned_state "$owned_pid")
            [[ "$state" == live ]] || break
            sleep 0.05
        done
    fi
    state=$(owned_state "$owned_pid")
    if [[ "$state" != gone && "$state" != stopped ]]; then
        printf 'owned_process_not_reaped pid=%s state=%s\n' "$owned_pid" "$state" >&2
        return 1
    fi
    wait "$owned_pid" 2>/dev/null || exit_code=$?
    printf '%s\n' "$exit_code" >"$lab/process-$owned_pid.exit-code"
}
request_fixture_realtime() {
    local pid=$1 policy priority hard_limit
    [[ $(id -un) == lab && $(stat -c %u "/proc/$pid") == 1000 &&
       $(owned_state "$pid") == live ]] || {
        echo 'Refusing RTKit request for an unverified owned fixture' >&2
        return 1
    }
    ps -p "$pid" -o pid,cls,rtprio,comm >"$lab/fixture-policy-before.txt"
    cp "/proc/$pid/limits" "$lab/fixture-limits-before.txt"
    hard_limit=$(awk '$1 == "Max" && $2 == "realtime" && $3 == "timeout" { print $5 }' \
        "$lab/fixture-limits-before.txt")
    [[ "$hard_limit" =~ ^[0-9]+$ ]] && (( hard_limit <= 200000 )) || {
        echo 'Owned fixture lacks the existing bounded RTKit runtime limit' >&2
        return 1
    }
    # The fixture serializes callbacks on its main loop, not an RT_PROCESS loop.
    # Grant only that exact owned main thread; never change system RTKit policy.
    timeout --signal=TERM --kill-after=1 5 busctl --system call \
        org.freedesktop.RealtimeKit1 /org/freedesktop/RealtimeKit1 \
        org.freedesktop.RealtimeKit1 MakeThreadRealtimeWithPID ttu "$pid" "$pid" 20 \
        >"$lab/fixture-rtkit-request.log" 2>&1 || return 1
    [[ $(owned_state "$pid") == live ]] || return 1
    read -r policy priority < <(ps -p "$pid" -o cls=,rtprio=)
    ps -L -p "$pid" -o pid,tid,cls,rtprio,comm >"$lab/fixture-policy-after.txt"
    [[ "$policy" == RR && "$priority" == 20 ]] || {
        echo 'Owned fixture main loop did not acquire RR priority 20' >&2
        return 1
    }
}
cleanup() {
    local original_status=$? cleanup_failed=0 owned_pid
    trap - EXIT
    trap '' INT TERM
    for owned_pid in "$sender_pid" "$engine_pid" "$fixture_pid" "$policy_pid" "$bus_pid" "$pipewire_pid"; do
        if ! stop_owned "$owned_pid"; then cleanup_failed=1; fi
    done
    if (( cleanup_failed )); then
        printf 'owned_process_cleanup_failed; evidence=%s original_status=%s\n' "$lab" "$original_status" >&2
        exit 1
    fi
    printf 'owned_processes_reaped; evidence=%s\n' "$lab"
    exit "$original_status"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT
dbus-daemon --session --nofork --print-address=1 >"$lab/dbus-address" 2>"$lab/dbus.log" &
bus_pid=$!
record_owned "$bus_pid" "$(command -v dbus-daemon)"
for _ in {1..100}; do [[ -s "$lab/dbus-address" ]] && break; sleep 0.02; done
IFS= read -r DBUS_SESSION_BUS_ADDRESS < "$lab/dbus-address"
export DBUS_SESSION_BUS_ADDRESS
PIPEWIRE_DEBUG='2,default:4,mod.rtp-source:5' PIPEWIRE_LOG_TIMESTAMP=monotonic \
    pipewire -c "$lab/pipewire.conf" >"$lab/pipewire.log" 2>&1 &
pipewire_pid=$!
record_owned "$pipewire_pid" "$(command -v pipewire)"
for _ in {1..100}; do [[ -S "$lab/runtime/pipewire-0" ]] && break; sleep 0.02; done
[[ -S "$lab/runtime/pipewire-0" ]] || exit 1
wireplumber >"$lab/wireplumber.log" 2>&1 &
policy_pid=$!
record_owned "$policy_pid" "$(command -v wireplumber)"
sleep 1
prlimit --rttime=200000:200000 -- "$lab/device-fixture" 2 2 "$lab/sink-monitor.f32" \
    >"$lab/device-fixture.json" 2>"$lab/device-fixture.log" &
fixture_pid=$!
record_owned "$fixture_pid" "$lab/device-fixture"
request_fixture_realtime "$fixture_pid"
for _ in {1..100}; do
    pw-dump >"$lab/before.json"
    if jq -e '[.[] | select(.type == "PipeWire:Interface:Node") | .info.props."node.name"] |
        index("io.mixrelay.lab-drift-rtp") != null and index("io.mixrelay.lab-device-sink") != null' "$lab/before.json" >/dev/null; then break; fi
    sleep 0.05
done
source_serial=$(jq -er '[.[] | select(.type == "PipeWire:Interface:Node" and .info.props."node.name" == "io.mixrelay.lab-drift-rtp") | .info.props."object.serial"] | if length == 1 then .[0] else error("RTP source missing or ambiguous") end' "$lab/before.json")
sink_serial=$(jq -er '[.[] | select(.type == "PipeWire:Interface:Node" and .info.props."node.name" == "io.mixrelay.lab-device-sink") | .info.props."object.serial"] | if length == 1 then .[0] else error("owned sink missing or ambiguous") end' "$lab/before.json")
jq -n --arg source "$source_serial" --arg sink "$sink_serial" '{rtpSourceSerial:$source,ownedSinkSerial:$sink}' >"$lab/native-targets.json"
MIXRELAY_CONFIG_DIR="$lab/engine-config" "$engine" >"$lab/engine.log" 2>&1 &
engine_pid=$!
record_owned "$engine_pid" "$engine"
api=http://mixrelay-engine/api/v1
for _ in {1..100}; do
    if curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --fail --silent --show-error --max-time 1 "$api/status" >"$lab/status-ready.json" 2>"$lab/status-ready-error.log"; then break; fi
    kill -0 "$engine_pid" 2>/dev/null || { echo 'Release engine exited before API readiness' >&2; exit 1; }
    sleep 0.1
done
[[ -s "$lab/status-ready.json" ]] || { echo 'Release engine API did not become ready' >&2; exit 1; }
curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --fail --silent --show-error --max-time 3 "$api/graph" >"$lab/graph-before.json"
jq --arg source 'io.mixrelay.lab-drift-rtp' --arg sink 'io.mixrelay.lab-device-sink' '
    .revision += 1 |
    .nodes = [
      {id:"drift-input",type:"audio_device_input",config:{selector:{match:"persistent_uid",uid:$source},outputChannels:2,gainDb:0}},
      {id:"drift-output",type:"audio_device_output",config:{selector:{match:"persistent_uid",uid:$sink},inputChannels:2}}
    ] |
    .edges = [{id:"drift-route",source:{nodeId:"drift-input",portId:"audio"},destination:{nodeId:"drift-output",portId:"audio"},channelMapping:{mode:"auto"}}]
' "$lab/graph-before.json" >"$lab/graph-request.json"
http_code=$(curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --silent --show-error --max-time 5 --output "$lab/graph-apply.json" --write-out '%{http_code}' \
    --request PUT --header 'content-type: application/json' --data-binary @"$lab/graph-request.json" "$api/graph")
[[ "$http_code" == 202 ]] || { printf 'Graph apply HTTP %s\n' "$http_code" >&2; exit 1; }
for _ in {1..100}; do
    curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --fail --silent --show-error --max-time 2 "$api/graph/status" >"$lab/graph-status.json"
    if jq -e --argjson revision "$(jq -r .revision "$lab/graph-request.json")" \
        '.activeRevision == $revision and .submittedRevision == $revision' "$lab/graph-status.json" >/dev/null; then break; fi
    sleep 0.1
done
jq -e --argjson revision "$(jq -r .revision "$lab/graph-request.json")" \
    '.activeRevision == $revision and .submittedRevision == $revision' "$lab/graph-status.json" >/dev/null || { echo 'Engine graph did not activate' >&2; exit 1; }
verify_links() {
    python3 - "$lab/graph-active.json" "$sink_serial" "$engine_pid" "$fixture_pid" <<'PY'
import json, sys
items = json.load(open(sys.argv[1]))
nodes = {item['id']: item['info']['props'] for item in items if item.get('type') == 'PipeWire:Interface:Node' and item.get('info', {}).get('props')}
clients = {item['id']: str(item['info']['props'].get('application.process.id')) for item in items if item.get('type') == 'PipeWire:Interface:Client' and item.get('info', {}).get('props')}
def owner(props): return str(props.get('application.process.id') or clients.get(props.get('client.id')))
by_serial = {str(props.get('object.serial')): node for node, props in nodes.items()}
sink = by_serial.get(sys.argv[2])
engine = {node for node, props in nodes.items() if owner(props) == sys.argv[3]}
fixture_source = {node for node, props in nodes.items() if owner(props) == sys.argv[4] and props.get('node.name') == 'io.mixrelay.lab-device-source'}
source = {node for node, props in nodes.items() if props.get('node.name') == 'io.mixrelay.lab-drift-rtp'}
links = {(item.get('info', {}).get('output-node-id'), item.get('info', {}).get('input-node-id')) for item in items if item.get('type') == 'PipeWire:Interface:Link'}
if sink is None or owner(nodes[sink]) != sys.argv[4] or len(fixture_source) != 1 or len(engine) < 2 or len(source) != 1: sys.exit(1)
if not any((node, sink) in links for node in engine): sys.exit(1)
if not any((next(iter(source)), node) in links for node in engine): sys.exit(1)
if any((node, sink) in links for node in fixture_source): sys.exit(1)
if any(target == sink and origin not in engine for origin, target in links): sys.exit(1)
PY
}
for _ in {1..100}; do
    pw-dump >"$lab/graph-active.json"
    if verify_links; then break; fi
    kill -0 "$fixture_pid" 2>/dev/null || { echo 'Owned sink capture exited before exact engine link' >&2; exit 1; }
    sleep 0.05
done
[[ -s "$lab/graph-active.json" ]] || exit 1
verify_links || { echo 'Exact RTP-to-engine-to-owned-sink graph did not link' >&2; exit 1; }
jq -n --argjson server "$pipewire_pid" --argjson engine "$engine_pid" \
    '{pipewirePid:$server,enginePid:$engine}' >"$lab/process-owners.json"
# RTKit grants are asynchronous. Observe the actual server clock driver, not
# merely an RT client thread, before starting the independent sample clock.
rt_observed=false
rt_deadline=$((SECONDS + 5))
while (( SECONDS < rt_deadline )); do
    ps -L -p "$pipewire_pid,$engine_pid,$fixture_pid" -o pid,tid,cls,rtprio,comm >"$lab/thread-policy.txt"
    if "$node_binary" "$analysis" --check-rt "$lab" >"$lab/scheduler-check.json"; then
        rt_observed=true
        break
    fi
    kill -0 "$pipewire_pid" 2>/dev/null && kill -0 "$engine_pid" 2>/dev/null || break
    sleep 0.1
done
[[ "$rt_observed" == true ]] || { echo 'Owned server/client data loops did not acquire RR priority 20' >&2; exit 1; }
cp "$lab/thread-policy.txt" "$lab/thread-policy-start.txt"
"$lab/drift-sender" "$port" "$ppm" "$seconds" >"$lab/sender.json" 2>"$lab/sender.log" &
sender_pid=$!
record_owned "$sender_pid" "$lab/drift-sender"
sleep 20
printf 'warmup_before_ns=%s\n' "$(python3 -c 'import time; print(time.monotonic_ns())')" >"$lab/phase-times.txt"
curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --fail --silent --show-error --max-time 3 "$api/status" >"$lab/engine-status-warmup.json"
printf 'warmup_after_ns=%s\n' "$(python3 -c 'import time; print(time.monotonic_ns())')" >>"$lab/phase-times.txt"
sleep 37
printf 'end_before_ns=%s\n' "$(python3 -c 'import time; print(time.monotonic_ns())')" >>"$lab/phase-times.txt"
curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --fail --silent --show-error --max-time 3 "$api/status" >"$lab/engine-status-end.json"
printf 'end_after_ns=%s\n' "$(python3 -c 'import time; print(time.monotonic_ns())')" >>"$lab/phase-times.txt"
ps -L -p "$pipewire_pid,$engine_pid,$fixture_pid" -o pid,tid,cls,rtprio,comm >"$lab/thread-policy.txt"
wait "$sender_pid"; sender_pid=''
curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --fail --silent --show-error --max-time 3 "$api/status" >"$lab/engine-status-after-sender.json"
stopped_fixture_pid=$fixture_pid
stop_owned "$fixture_pid"; fixture_pid=''
[[ $(<"$lab/process-$stopped_fixture_pid.exit-code") == 0 ]] || { echo 'Owned sink capture did not finalize cleanly' >&2; exit 1; }
jq -e '.sinkPcmCapture | .invalidCallbacks == 0 and .capturedFrames > 0 and .rawBytes == (.capturedFrames * 8)' \
    "$lab/device-fixture.json" >/dev/null || { echo 'Owned sink capture receipt is invalid' >&2; exit 1; }
[[ $(stat -c %s "$lab/sink-monitor.f32") == "$(jq -r '.sinkPcmCapture.rawBytes' "$lab/device-fixture.json")" ]] || {
    echo 'Owned sink capture byte count differs from its receipt' >&2; exit 1;
}
pw-dump >"$lab/before-engine-shutdown.json"
stopped_engine_pid=$engine_pid
stop_owned "$engine_pid"; engine_pid=''
[[ $(<"$lab/process-$stopped_engine_pid.exit-code") == 0 ]] || { echo 'Release engine did not shut down cleanly' >&2; exit 1; }
[[ ! -e "$engine_socket" && ! -L "$engine_socket" ]] || { echo 'Release engine left the API socket' >&2; exit 1; }
if curl --unix-socket "$engine_socket" --header 'Host: mixrelay-engine' --silent --max-time 1 "$api/status" >/dev/null; then
    echo 'Release engine control socket is still connectable' >&2; exit 1
fi
pw-dump >"$lab/after.json"
jq -e --arg pid "$stopped_engine_pid" '[.[] | select((.info.props."application.process.id" | tostring) == $pid)] | length == 0' "$lab/after.json" >/dev/null || {
    echo 'Release engine left owned PipeWire objects' >&2; exit 1;
}
"$node_binary" "$analysis" --warmup=20 "$lab" >"$lab/analysis.json"
cat "$lab/analysis.json"
