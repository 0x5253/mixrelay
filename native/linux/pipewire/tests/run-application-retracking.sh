#!/usr/bin/env bash
set -euo pipefail
test_launcher_pid=''
test_session_id=''
test_session_verified=0
cleanup_failed=0

resolve_cargo_target_dir() {
    local workspace=$1
    local requested=${2:-}
    local requested_entry
    local canonical_workspace
    local canonical_target
    local workspace_validation_name
    local workspace_lab_parent
    local target_validation_dir
    local target_validation_name
    local target_lab_parent

    canonical_workspace=$(realpath "$workspace") || return 2
    if [[ -z "$requested" ]]; then
        printf '%s/target\n' "$canonical_workspace"
        return 0
    fi

    [[ "$requested" == /* && -d "$requested" ]] || return 2
    requested_entry=$requested
    while [[ "$requested_entry" != / && "$requested_entry" == */ ]]; do
        requested_entry=${requested_entry%/}
    done
    [[ ! -L "$requested_entry" ]] || return 2
    canonical_target=$(realpath "$requested_entry") || return 2
    [[ "$canonical_target" == "$requested_entry" && "${canonical_target##*/}" == target ]] || return 2
    workspace_validation_name=${canonical_workspace##*/}
    workspace_lab_parent=${canonical_workspace%/*}
    target_validation_dir=${canonical_target%/*}
    target_validation_name=${target_validation_dir##*/}
    target_lab_parent=${target_validation_dir%/*}
    [[ "$workspace_validation_name" == mixrelay-integration-* &&
       "${workspace_lab_parent##*/}" == .lab &&
       "$target_validation_name" == mixrelay-integration-* &&
       "$target_lab_parent" == "$workspace_lab_parent" ]] || return 2
    printf '%s\n' "$canonical_target"
}

validate_evidence_root() {
    local workspace=$1
    [[ ! -L "$workspace/target" && ! -L "$workspace/target/native-tests" ]]
}

test_session_members() {
    # A fixture that deliberately calls setsid escapes this owned session and
    # is outside the bounded cleanup scope; ordinary fixture descendants keep it.
    [[ "$test_session_verified" == 1 && "$test_session_id" =~ ^[1-9][0-9]*$ ]] || return 0
    ps -eo pid=,sid=,stat= 2>/dev/null |
        awk -v session_id="$test_session_id" '$2 == session_id && $1 ~ /^[1-9][0-9]*$/ && $3 !~ /^Z/ { print $1 }'
}

cleanup_test_session() {
    local member_pid
    local -a members=()
    local -i attempt=0

    [[ "$test_session_verified" == 1 ]] || return 0
    mapfile -t members < <(test_session_members)
    for member_pid in "${members[@]}"; do
        kill -TERM "$member_pid" 2>/dev/null || true
    done
    for ((attempt = 0; attempt < 100; attempt++)); do
        if [[ -z "$(test_session_members)" ]]; then
            test_session_id=''
            test_session_verified=0
            return 0
        fi
        sleep 0.02
    done
    mapfile -t members < <(test_session_members)
    for member_pid in "${members[@]}"; do
        kill -KILL "$member_pid" 2>/dev/null || true
    done
    for ((attempt = 0; attempt < 100; attempt++)); do
        if [[ -z "$(test_session_members)" ]]; then
            test_session_id=''
            test_session_verified=0
            return 0
        fi
        sleep 0.02
    done
    return 1
}

stop_owned_pid() {
    local pid=${1:-}
    local -i attempt=0

    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 0
    kill -TERM "$pid" 2>/dev/null || true
    for ((attempt = 0; attempt < 100; attempt++)); do
        if ! kill -0 "$pid" 2>/dev/null; then
            wait "$pid" 2>/dev/null || true
            return 0
        fi
        sleep 0.02
    done
    kill -KILL "$pid" 2>/dev/null || true
    for ((attempt = 0; attempt < 100; attempt++)); do
        if ! kill -0 "$pid" 2>/dev/null; then
            wait "$pid" 2>/dev/null || true
            return 0
        fi
        sleep 0.02
    done
    return 1
}

start_test_session() {
    local marker=$1
    shift
    local marker_session=''

    rm -f -- "$marker"
    # The child expands its own positional parameters after entering the session.
    # shellcheck disable=SC2016
    setsid --wait bash -c '
        marker=$1
        shift
        printf "%s\n" "$$" > "$marker"
        exec "$@"
    ' _ "$marker" "$@" &
    test_launcher_pid=$!
    for _ in {1..100}; do
        if [[ -s "$marker" ]]; then
            marker_session=$(<"$marker")
            break
        fi
        sleep 0.02
    done
    [[ "$marker_session" =~ ^[1-9][0-9]*$ ]] || return 1
    test_session_id=$marker_session
    test_session_verified=1
    return 0
}

cleanup() {
    local cleanup_ok=1
    if ! cleanup_test_session; then cleanup_ok=0; fi
    if ! stop_owned_pid "$inventory_pid"; then cleanup_ok=0; fi
    if ! stop_owned_pid "$profiler_pid"; then cleanup_ok=0; fi
    if ! stop_owned_pid "$policy_pid"; then cleanup_ok=0; fi
    if ! stop_owned_pid "$pipewire_pid"; then cleanup_ok=0; fi
    if ! stop_owned_pid "$bus_pid"; then cleanup_ok=0; fi
    if [[ -n "$test_launcher_pid" ]]; then
        if ! stop_owned_pid "$test_launcher_pid"; then cleanup_ok=0; fi
        test_launcher_pid=''
    fi
    if (( cleanup_ok )); then
        printf 'owned_processes_stopped; evidence=%s\n' "$lab"
    else
        cleanup_failed=1
        printf 'owned_process_cleanup_failed; evidence=%s\n' "$lab" >&2
    fi
}

install_cleanup_traps() {
    trap cleanup EXIT
    trap 'exit 143' TERM
    trap 'exit 130' INT
}

main() {
[[ $(uname -s) == Linux && $(id -u) != 0 ]] || exit 2
command -v ps >/dev/null && command -v setsid >/dev/null && command -v realpath >/dev/null || exit 2
workspace=${1:?Expected isolated source directory}
graph_rate=${MIXRELAY_LAB_GRAPH_RATE:-48000}
[[ "$graph_rate" == 44100 || "$graph_rate" == 48000 ]] || exit 2
source_dispatch=${MIXRELAY_LAB_SOURCE_DISPATCH:-main}
[[ "$source_dispatch" == main || "$source_dispatch" == data ]] || exit 2
test_name=${MIXRELAY_LAB_TEST_NAME:-native_application_stream_relaunch_recovers_pcm_without_graph_reapply}
test_profile=${MIXRELAY_LAB_TEST_PROFILE:-dev}
[[ "$test_profile" == dev || "$test_profile" == release ]] || exit 2
test_profile_args=()
artifact_profile=debug
if [[ "$test_profile" == release ]]; then
    test_profile_args+=(--release)
    artifact_profile=release
fi
observe_buffers=${MIXRELAY_LAB_BUFFER_OBSERVER:-0}
[[ "$observe_buffers" == 0 || "$observe_buffers" == 1 ]] || exit 2
if [[ "$observe_buffers" == 1 ]]; then
    export MIXRELAY_LAB_PROCESS_OBSERVER=1
else
    unset MIXRELAY_LAB_PROCESS_OBSERVER
fi
profile_graph=$observe_buffers
if [[ "$test_name" == native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state ||
      "$test_name" == native_driver_32_64_pcm_continues_after_graph_client_exit ]]; then
    profile_graph=0
fi
if [[ "$observe_buffers" == 1 ]]; then
    [[ ( "$test_name" == native_application_continuity_has_independent_capture_evidence ||
         "$test_name" == native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state ||
         "$test_name" == native_driver_32_64_pcm_continues_after_graph_client_exit ) &&
       "$graph_rate" == 48000 && "${MIXRELAY_LAB_SOURCE_RATE:-48000}" == 48000 &&
       "${MIXRELAY_LAB_LIFECYCLE_TEST:-0}" == 0 ]] || exit 2
fi
export MIXRELAY_LAB_SOURCE_DISPATCH="$source_dispatch"
[[ "$workspace" == /*/.lab/mixrelay-integration-* && -f "$workspace/Cargo.toml" ]] || exit 2
workspace=$(realpath "$workspace") || exit 2
[[ "$workspace" == /*/.lab/mixrelay-integration-* && -f "$workspace/Cargo.toml" ]] || exit 2
cargo_target_dir=$(resolve_cargo_target_dir "$workspace" "${CARGO_TARGET_DIR:-}") || {
    printf 'CARGO_TARGET_DIR must be an existing nonsymlink target directory in a mixrelay-integration-* sibling under this validation root .lab parent\n' >&2
    exit 2
}
validate_evidence_root "$workspace" || {
    printf 'workspace/target and workspace/target/native-tests must not be symlinks\n' >&2
    exit 2
}
mkdir -p "$workspace/target/native-tests"
lab=$(mktemp -d "$workspace/target/native-tests/app-lab-XXXXXX")
printf 'isolated_lab=%s\n' "$lab"
printf 'test_profile=%s\n' "$test_profile"
if [[ "$observe_buffers" == 1 ]]; then
    printf 'measurement_mode=diagnostic profiler_enabled=%s\n' "$profile_graph"
else
    printf 'measurement_mode=normal profiler_enabled=0\n'
fi
# PipeWire 1.x also creates a -manager socket. Linux sockaddr_un includes the
# terminating NUL in its 108-byte limit; count bytes, not locale characters.
socket_bytes=$(printf '%s' "$lab/runtime/pipewire-0-manager" | wc -c)
(( socket_bytes < 108 )) || { printf 'Private lab socket path is too long; use a shorter workspace mount\n' >&2; exit 2; }
mkdir -p "$lab/runtime" "$lab/config" "$lab/state" "$lab/data"
chmod 700 "$lab/runtime"
fixture=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
policy_hook=none
if [[ "$test_name" == native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state ]]; then
    policy_hook=source-mute
fi
bash "$fixture/configure-application-policy.sh" "$fixture" "$lab" "$policy_hook"
cp "$fixture/application-lab-pipewire.conf" "$lab/pipewire.conf"
sed -i "s/default.clock.rate = 48000/default.clock.rate = $graph_rate/" "$lab/pipewire.conf"
export XDG_RUNTIME_DIR="$lab/runtime" XDG_CONFIG_HOME="$lab/config" XDG_STATE_HOME="$lab/state" XDG_DATA_HOME="$lab/data"
export MIXRELAY_IPC_DIR="$lab/ipc"
export WIREPLUMBER_CONFIG_DIR="$lab/config/wireplumber"
if [[ "$policy_hook" == source-mute ]]; then
    export WIREPLUMBER_DATA_DIR="$lab/data/wireplumber"
    printf 'forward_hook_data_dir=%s\n' "$WIREPLUMBER_DATA_DIR"
    cat "$lab/source-mute-hook.sha256"
fi
export CARGO_TARGET_DIR="$cargo_target_dir"
unset PIPEWIRE_REMOTE PIPEWIRE_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS
cd "$workspace"
cargo build --locked "${test_profile_args[@]}" -p mixrelay-linux-pipewire --example application-source
cp "$cargo_target_dir/$artifact_profile/examples/application-source" "$lab/application-fixture"
if [[ "$test_name" == native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state ||
      "$test_name" == native_application_graph_continuity_preserves_480k_frames_through_engine ]]; then
    [[ "${MIXRELAY_LAB_SOURCE_RATE:-48000}" == 48000 ]] || exit 2
    cargo build --locked "${test_profile_args[@]}" -p mixrelay-linux-pipewire --example application-mute-sink
    cp "$cargo_target_dir/$artifact_profile/examples/application-mute-sink" "$lab/mute-sink-fixture"
fi
if [[ "$test_name" == native_application_graph_continuity_preserves_480k_frames_through_engine ]]; then
    [[ "${MIXRELAY_LAB_LIFECYCLE_TEST:-0}" == 0 && "$observe_buffers" == 0 ]] || exit 2
    export MIXRELAY_LAB_SOURCE_RATE=48000
fi
observer_args=()
if [[ "$observe_buffers" == 1 ]]; then
    cargo build --locked -p mixrelay-linux-pipewire-observer --release
    host=$(rustc -vV | sed -n 's/^host: //p')
    [[ -n "$host" && "$workspace" != *"'"* && "$cargo_target_dir" != *"'"* ]] || exit 2
    # Preload only the test binary and its children, not Cargo or the daemon.
    observer_args=(--config "target.$host.runner=['env','LD_PRELOAD=$cargo_target_dir/release/libmixrelay_linux_pipewire_observer.so']")
fi
cc -Wall -Wextra -Werror "$fixture/MRDeviceFixture.c" $(pkg-config --cflags --libs libpipewire-0.3) -lm -o "$lab/device-fixture"
export MIXRELAY_LAB_DEVICE_FIXTURE="$lab/device-fixture"
pipewire_pid=''
policy_pid=''
bus_pid=''
profiler_pid=''
inventory_pid=''
install_cleanup_traps
dbus-daemon --session --nofork --print-address=1 >"$lab/dbus-address" 2>"$lab/dbus.log" &
bus_pid=$!
for _ in {1..100}; do [[ -s "$lab/dbus-address" ]] && break; sleep 0.02; done
IFS= read -r DBUS_SESSION_BUS_ADDRESS < "$lab/dbus-address"
export DBUS_SESSION_BUS_ADDRESS
pipewire -c "$lab/pipewire.conf" >"$lab/pipewire.log" 2>&1 &
pipewire_pid=$!
for _ in {1..100}; do [[ -S "$lab/runtime/pipewire-0" ]] && break; sleep 0.02; done
[[ -S "$lab/runtime/pipewire-0" ]] || exit 1
wireplumber >"$lab/wireplumber.log" 2>&1 &
policy_pid=$!
sleep 1
pw-dump >"$lab/before.json"
# Retain native formats, link states and node transitions while the test owns
# them. A final snapshot after a panic would only see already-destroyed nodes.
pw-dump --monitor >"$lab/native-graph.jsonl" 2>"$lab/native-graph.log" &
inventory_pid=$!
for _ in {1..100}; do [[ -s "$lab/native-graph.jsonl" ]] && break; sleep 0.02; done
[[ -s "$lab/native-graph.jsonl" ]] && kill -0 "$inventory_pid"
# A profiler subscriber enables native RT profiling work. Forward handoff
# journals do not require it; keep it with the independent-capture diagnostic,
# outside normal PCM qualification runs.
if [[ "$profile_graph" == 1 ]]; then
    profiler_output=native-profiler.tsv
    (
        cd "$lab"
        exec pw-profiler -o "$profiler_output" >native-profiler.log 2>&1
    ) &
    profiler_pid=$!
    for _ in {1..100}; do [[ -f "$lab/$profiler_output" ]] && break; sleep 0.02; done
    [[ -f "$lab/$profiler_output" ]] && kill -0 "$profiler_pid"
fi
if [[ ${MIXRELAY_LAB_LIFECYCLE_TEST:-0} == 1 ]]; then
    cargo build --locked --manifest-path "$workspace/Cargo.toml" -p mixrelay-linux-pipewire --release --target-dir "$cargo_target_dir"
    cc -O2 -std=c11 -D_GNU_SOURCE -Wall -Wextra -Werror -pthread \
        $(pkg-config --cflags libpipewire-0.3) -I "$workspace/native/linux/pipewire" -I "$workspace/native/pcm-ring/include" \
        "$workspace/native/linux/pipewire/tests/MRLifecycleFixture.c" \
        "$cargo_target_dir/release/libmixrelay_linux_pipewire.a" -ldl \
        $(pkg-config --libs libpipewire-0.3) -lm -o "$lab/lifecycle-fixture"
    test_command=("$lab/lifecycle-fixture")
elif [[ "$test_name" == native_device_boundary_probe ]]; then
    [[ "$observe_buffers" == 0 && "$profile_graph" == 0 ]] || exit 2
    cargo build --locked "${test_profile_args[@]}" -p mixrelay-engine-service --bin mixrelay-service
    test_command=(node "$workspace/scripts/lab/linux-device-probe.mjs"
        "$cargo_target_dir/$artifact_profile/mixrelay-service" "$lab/device-fixture" "$lab/device-evidence")
elif [[ "$test_name" == native_pipewire_connection_lifecycle || "$test_name" == native_graph_driver_drains_before_processor_drop_and_releases_resources ]]; then
    test_command=(env "MIXRELAY_LAB_APPLICATION_FIXTURE=$lab/application-fixture" cargo test --locked --features native-lab "${test_profile_args[@]}" -p mixrelay-linux-pipewire --lib "$test_name" -- --nocapture --test-threads=1)
elif [[ "$test_name" == native_driver_* ]]; then
    mapfile -t listed_tests < <(cargo test --locked --features native-lab "${test_profile_args[@]}" -p mixrelay-linux-driver --lib -- --list |
        awk -v test_name="$test_name" '{ n = split($0, parts, "::"); if (parts[n] == test_name ": test") print }')
    [[ ${#listed_tests[@]} == 1 ]] || {
        printf 'Native Driver test must be listed exactly once; found %s\n' "${#listed_tests[@]}" >&2
        exit 1
    }
    test_filter=${listed_tests[0]%: test}
    printf 'driver_native_test=%s\ndriver_native_listed_count=1\n' "$test_filter"
    test_command=(env "MIXRELAY_LAB_APPLICATION_FIXTURE=$lab/application-fixture" cargo "${observer_args[@]}" test --locked --features native-lab "${test_profile_args[@]}" -p mixrelay-linux-driver --lib "$test_filter" -- --exact --nocapture --test-threads=1)
elif [[ "$test_name" == native_linux_service_* ]]; then
    test_command=(env "MIXRELAY_LAB_APPLICATION_FIXTURE=$lab/application-fixture" cargo test --locked --features native-lab "${test_profile_args[@]}" -p mixrelay-engine-service --test linux_service_lifecycle "$test_name" -- --nocapture --test-threads=1)
else
    test_environment=("MIXRELAY_LAB_APPLICATION_FIXTURE=$lab/application-fixture")
    if [[ "$test_name" == native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state ||
          "$test_name" == native_application_graph_continuity_preserves_480k_frames_through_engine ]]; then
        test_environment+=("MIXRELAY_LAB_APPLICATION_MUTE_SINK=$lab/mute-sink-fixture")
    fi
    test_filter=$test_name
    exact_args=()
    if [[ "$test_name" == native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state ]]; then
        mapfile -t listed_tests < <(cargo test --locked --features native-lab "${test_profile_args[@]}" -p mixrelay-linux-engine --lib -- --list |
            awk -v test_name="$test_name" '$0 ~ (test_name ": test$") { print }')
        [[ ${#listed_tests[@]} == 1 ]] || {
            printf 'Forward/WP ACK test must be listed exactly once; found %s\n' "${#listed_tests[@]}" >&2
            exit 1
        }
        test_filter=${listed_tests[0]%: test}
        exact_args=(--exact)
        printf 'forward_native_test=%s\nforward_native_listed_count=1\n' "$test_filter"
    fi
    test_command=(env "${test_environment[@]}" cargo "${observer_args[@]}" test --locked --features native-lab "${test_profile_args[@]}" -p mixrelay-linux-engine --lib "$test_filter" -- "${exact_args[@]}" --nocapture --test-threads=1)
fi
test_exit_code=0
if ! start_test_session "$lab/test-session-id" "${test_command[@]}"; then
    test_exit_code=1
else
    wait "$test_launcher_pid" || test_exit_code=$?
    test_launcher_pid=''
fi
if ! cleanup_test_session; then cleanup_failed=1; fi
post_test_code=0
if (( test_exit_code == 0 )); then
    if [[ -n "$profiler_pid" ]]; then
        kill -0 "$profiler_pid" 2>/dev/null || post_test_code=$?
    fi
    kill -0 "$inventory_pid" 2>/dev/null || post_test_code=$?
    if (( post_test_code == 0 )); then pw-dump >"$lab/after.json" || post_test_code=$?; fi
fi
trap - EXIT
cleanup
if (( test_exit_code != 0 )); then exit "$test_exit_code"; fi
if (( post_test_code != 0 )); then exit "$post_test_code"; fi
if (( cleanup_failed != 0 )); then exit 1; fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
