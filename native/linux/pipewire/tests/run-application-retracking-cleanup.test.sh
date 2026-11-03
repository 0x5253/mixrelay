#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s) != Linux || $(id -u) == 0 ]]; then
    printf 'UNAVAILABLE: cleanup regression requires a non-root Linux host\n' >&2
    exit 2
fi
if ! command -v ps >/dev/null || ! command -v setsid >/dev/null; then
    printf 'UNAVAILABLE: cleanup regression requires ps and setsid\n' >&2
    exit 2
fi

fixture=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# Sourcing is safe because the wrapper only calls main when executed directly.
# shellcheck disable=SC1091,SC2034
source "$fixture/run-application-retracking.sh"

repo_root=$(cd -- "$fixture/../../../.." && pwd)
artifact_root="$repo_root/target/test-artifacts"
mkdir -p -- "$artifact_root"
root=''
for attempt in {0..127}; do
    candidate="$artifact_root/mixrelay-cleanup-test-${BASHPID}-${attempt}"
    if mkdir -- "$candidate" 2>/dev/null; then
        root=$candidate
        break
    fi
done
if [[ -z "$root" ]]; then
    printf 'FAIL: could not create checkout-local fixture directory\n' >&2
    exit 1
fi
unrelated_pid=''
outer_pid=''
active_outer_marker=''
test_launcher_pid=''
# These globals are consumed by functions sourced from the wrapper.
# shellcheck disable=SC2034
test_session_id=''
# shellcheck disable=SC2034
test_session_verified=0

process_running() {
    local pid=$1
    local state
    state=$(ps -o stat= -p "$pid" 2>/dev/null | tr -d '[:space:]')
    [[ -n "$state" && "$state" != Z* ]]
}

finish() {
    set +e
    cleanup_test_session
    if [[ -n "$test_launcher_pid" ]]; then
        kill -KILL "$test_launcher_pid" 2>/dev/null || true
        wait "$test_launcher_pid" 2>/dev/null || true
    fi
    if [[ -n "$unrelated_pid" ]]; then
        kill -KILL "$unrelated_pid" 2>/dev/null || true
        wait "$unrelated_pid" 2>/dev/null || true
    fi
    if [[ -n "$outer_pid" ]]; then
        kill -KILL "$outer_pid" 2>/dev/null || true
        wait "$outer_pid" 2>/dev/null || true
    fi
    if [[ -n "$active_outer_marker" && -s "$active_outer_marker" ]]; then
        test_session_id=$(<"$active_outer_marker")
        test_session_verified=1
        cleanup_test_session || true
    fi
    rm -rf -- "$root"
}
trap finish EXIT

run_outer_signal_case() {
    local signal_name=$1
    local expected_exit=$2
    local marker="$root/outer-$signal_name-session-id"
    local child_marker="$root/outer-$signal_name-child-pid"
    local ready_marker="$root/outer-$signal_name-ready"
    local log="$root/outer-$signal_name.log"
    local child_pid=''
    local outer_exit=0
    # The outer shell expands these parameters when it runs the source string.
    # shellcheck disable=SC2016
    local outer_code='
source "$1"
lab=$2
inventory_pid=""
profiler_pid=""
policy_pid=""
pipewire_pid=""
bus_pid=""
install_cleanup_traps
start_test_session "$3" bash -c "child_marker=\$1; sleep 30 & child=\$!; printf \"%s\\n\" \"\$child\" > \"\$child_marker\"; wait" _ "$4"
for _ in {1..100}; do
    [[ -s "$4" ]] && break
    sleep 0.02
done
printf "%s\\n" ready > "$5"
wait "$test_launcher_pid" || true
'

    outer_pid=''
    active_outer_marker=$marker
    env --default-signal=INT bash -c "$outer_code" _ "$fixture/run-application-retracking.sh" "$root" "$marker" "$child_marker" "$ready_marker" >"$log" 2>&1 &
    outer_pid=$!
    for _ in {1..100}; do
        [[ -s "$ready_marker" ]] && break
        sleep 0.02
    done
    [[ -s "$ready_marker" && -s "$child_marker" ]] || { printf 'FAIL: outer %s readiness marker was not created\n' "$signal_name" >&2; return 1; }
    child_pid=$(<"$child_marker")
    kill -"$signal_name" "$outer_pid"
    for _ in {1..100}; do
        process_running "$outer_pid" || break
        sleep 0.02
    done
    if process_running "$outer_pid"; then
        kill -KILL "$outer_pid" 2>/dev/null || true
    fi
    wait "$outer_pid" || outer_exit=$?
    if [[ -s "$marker" ]]; then
        test_session_id=$(<"$marker")
        test_session_verified=1
        cleanup_test_session || true
    fi
    outer_pid=''
    active_outer_marker=''
    [[ "$outer_exit" == "$expected_exit" ]] || {
        printf 'FAIL: outer %s exit was %s\n' "$signal_name" "$outer_exit" >&2
        return 1
    }
    for _ in {1..100}; do
        process_running "$child_pid" || break
        sleep 0.02
    done
    process_running "$child_pid" && {
        printf 'FAIL: outer %s cleanup left owned child running\n' "$signal_name" >&2
        return 1
    }
    process_running "$unrelated_pid" || {
        printf 'FAIL: unrelated process was killed during outer %s cleanup\n' "$signal_name" >&2
        return 1
    }
    grep -q 'owned_processes_stopped' "$log" || {
        printf 'FAIL: outer %s did not report bounded cleanup\n' "$signal_name" >&2
        return 1
    }
    printf 'PASS: outer %s exit=%s cleanup ran unrelated process survived\n' "$signal_name" "$outer_exit"
}

# shellcheck disable=SC2034
run_case() {
    local mode=$1
    local marker="$root/$mode-session-id"
    local child_marker="$root/$mode-child-pid"
    local test_exit_code=0
    local child_pid=''

    test_session_id=''
    test_session_verified=0
    # The child expands its own positional parameters after entering the session.
    # shellcheck disable=SC2016
    start_test_session "$marker" bash -c '
        child_marker=$1
        mode=$2
        (trap "" TERM; sleep 30) &
        child=$!
        printf "%s\n" "$child" > "$child_marker"
        case "$mode" in
            normal) exit 0 ;;
            term) kill -TERM "$$" ;;
            kill) kill -KILL "$$" ;;
            *) exit 2 ;;
        esac
    ' _ "$child_marker" "$mode"

    for _ in {1..100}; do
        [[ -s "$child_marker" ]] && break
        sleep 0.02
    done
    [[ -s "$child_marker" ]] || { printf 'FAIL: %s child marker was not created\n' "$mode" >&2; return 1; }
    child_pid=$(<"$child_marker")
    [[ "$child_pid" =~ ^[1-9][0-9]*$ ]] || { printf 'FAIL: %s child PID is invalid\n' "$mode" >&2; return 1; }

    wait "$test_launcher_pid" || test_exit_code=$?
    test_launcher_pid=''
    case "$mode" in
        normal) [[ "$test_exit_code" == 0 ]] || { printf 'FAIL: normal exit was %s\n' "$test_exit_code" >&2; return 1; } ;;
        term) [[ "$test_exit_code" == 143 ]] || { printf 'FAIL: TERM exit was %s\n' "$test_exit_code" >&2; return 1; } ;;
        kill) [[ "$test_exit_code" == 137 ]] || { printf 'FAIL: SIGKILL exit was %s\n' "$test_exit_code" >&2; return 1; } ;;
    esac
    cleanup_test_session || { printf 'FAIL: %s owned session did not terminate\n' "$mode" >&2; return 1; }
    [[ -z "$(test_session_members)" ]] || { printf 'FAIL: %s owned session still has members\n' "$mode" >&2; return 1; }
    for _ in {1..100}; do
        process_running "$child_pid" || break
        sleep 0.02
    done
    process_running "$child_pid" && { printf 'FAIL: %s owned child survived cleanup\n' "$mode" >&2; return 1; }
    process_running "$unrelated_pid" || { printf 'FAIL: unrelated process was killed during %s cleanup\n' "$mode" >&2; return 1; }
    printf 'PASS: %s exit=%s owned child cleaned unrelated process survived\n' "$mode" "$test_exit_code"
}

sleep 30 &
unrelated_pid=$!
for mode in normal term kill; do
    run_case "$mode"
done
run_outer_signal_case TERM 143
run_outer_signal_case INT 130
