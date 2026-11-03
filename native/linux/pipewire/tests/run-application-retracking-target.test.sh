#!/usr/bin/env bash
set -euo pipefail

if ! command -v realpath >/dev/null; then
    printf 'UNAVAILABLE: target-directory regression requires realpath\n' >&2
    exit 2
fi

fixture=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck disable=SC1091
source "$fixture/run-application-retracking.sh"

temporary_root=$(mktemp -d)
cleanup_test_tmp() {
    rm -rf -- "$temporary_root"
}
trap cleanup_test_tmp EXIT

workspace="$temporary_root/.lab/mixrelay-integration-target-test"
outside="$temporary_root/outside"
mkdir -p -- "$workspace" "$outside"
touch "$workspace/Cargo.toml"
canonical_workspace=$(realpath "$workspace")
workspace=$canonical_workspace
outside=$(realpath "$outside")

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

expect_target() {
    local label=$1
    local requested=$2
    local expected=$3
    local actual

    actual=$(resolve_cargo_target_dir "$workspace" "$requested") || fail "$label unexpectedly rejected"
    [[ "$actual" == "$expected" ]] || fail "$label returned '$actual', expected '$expected'"
}

expect_rejected() {
    local label=$1
    local requested=$2
    if resolve_cargo_target_dir "$workspace" "$requested" >/dev/null 2>&1; then
        fail "$label unexpectedly accepted"
    fi
}

expect_target 'default target' '' "$canonical_workspace/target"
[[ ! -e "$workspace/target" ]] || fail 'default resolution created a target directory'

mkdir -- "$workspace/target"
expect_target 'current snapshot target override' "$workspace/target" "$workspace/target"

sibling_workspace="${workspace%/*}/mixrelay-integration-target-cache"
cache="$sibling_workspace/target"
mkdir -p -- "$cache"
printf 'preserve-me\n' > "$cache/cache-sentinel"
expect_target 'sibling validation snapshot cache' "$cache" "$cache"
[[ "$(<"$cache/cache-sentinel")" == preserve-me ]] || fail 'override resolution modified its cache'

ln -s -- "$cache" "$workspace/target-link"
expect_rejected 'symlink override' "$workspace/target-link"
expect_rejected 'symlink override with trailing slash' "$workspace/target-link/"
ln -s -- "$sibling_workspace" "$workspace/sibling-link"
expect_rejected 'symlinked cache ancestor' "$workspace/sibling-link/target"
expect_rejected 'relative override' 'relative-target'
expect_rejected 'missing override' "$workspace/missing-target"
expect_rejected 'wrong target basename' "$workspace/target/native-tests"
wrong_sibling_target="${workspace%/*}/other-snapshot/target"
mkdir -p -- "$wrong_sibling_target"
expect_rejected 'wrong sibling basename' "$wrong_sibling_target"
outside_validation_target="$temporary_root/other-lab/.lab/mixrelay-integration-outside/target"
mkdir -p -- "$outside_validation_target"
expect_rejected 'different validation parent' "$outside_validation_target"
expect_rejected 'not a target directory' "$outside"

ln -s -- "$outside" "$workspace/target/native-tests"
if validate_evidence_root "$workspace"; then
    fail 'symlinked native-tests evidence root was accepted'
fi
rm -f -- "$workspace/target/native-tests"
rmdir -- "$workspace/target"
ln -s -- "$outside" "$workspace/target"
if validate_evidence_root "$workspace"; then
    fail 'symlinked workspace target root was accepted'
fi
rm -f -- "$workspace/target"
mkdir -p -- "$workspace/target/native-tests/cache"
expect_rejected 'evidence-root override' "$workspace/target/native-tests"
expect_rejected 'nested evidence-root override' "$workspace/target/native-tests/cache"
[[ "$(<"$cache/cache-sentinel")" == preserve-me ]] || fail 'rejected paths modified the reusable cache'

grep -Fq "export CARGO_TARGET_DIR=\"\$cargo_target_dir\"" "$fixture/run-application-retracking.sh" ||
    fail 'Cargo does not receive the resolved target directory'
grep -Fq "cp \"\$cargo_target_dir/\$artifact_profile/examples/application-source\"" "$fixture/run-application-retracking.sh" ||
    fail 'application fixture lookup ignores the resolved target directory'
grep -Fq "cp \"\$cargo_target_dir/\$artifact_profile/examples/application-mute-sink\"" "$fixture/run-application-retracking.sh" ||
    fail 'mute observer lookup ignores the resolved target directory'
grep -Fq "LD_PRELOAD=\$cargo_target_dir/release/libmixrelay_pipewire_observer.so" "$fixture/run-application-retracking.sh" ||
    fail 'observer preload lookup ignores the resolved target directory'
grep -Fq "\"\$cargo_target_dir/release/libmixrelay_pipewire.a\"" "$fixture/run-application-retracking.sh" ||
    fail 'lifecycle library lookup ignores the resolved target directory'
grep -Fq "mktemp -d \"\$workspace/target/native-tests/app-lab-XXXXXX\"" "$fixture/run-application-retracking.sh" ||
    fail 'runtime evidence location is no longer workspace-owned'

if grep -Eq "\\\$workspace/target/(debug|release)/" "$fixture/run-application-retracking.sh"; then
    fail 'a Cargo artifact lookup still hardcodes workspace/target'
fi

check_fixture_profile() (
    # These values are consumed by the extracted runner commands below.
    # shellcheck disable=SC2034
    test_profile=$1
    expected_profile=$2
    expected_release=$3
    # shellcheck disable=SC2034
    test_name=native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state
    # shellcheck disable=SC2034
    graph_rate=48000
    # shellcheck disable=SC2034
    cargo_target_dir=$cache
    lab="$workspace/target/native-tests/profile-test"
    artifact_profile=debug
    eval "$(sed -n '/^test_profile_args=()/,/^observe_buffers=/p' "$fixture/run-application-retracking.sh" | sed '$d')"
    [[ "$artifact_profile" == "$expected_profile" ]] || fail 'wrong artifact profile'
    cargo() {
        [[ "$*" == "build --locked ${expected_release}-p mixrelay-pipewire --example application-source" ||
           "$*" == "build --locked ${expected_release}-p mixrelay-pipewire --example application-mute-sink" ]] ||
            fail 'fixture Cargo arguments do not match selected test profile'
        printf 'cargo:%s\n' "$*"
    }
    cp() {
        [[ "$1" == "$cache/$expected_profile/examples/application-source" ||
           "$1" == "$cache/$expected_profile/examples/application-mute-sink" ]] ||
            fail 'fixture copy path does not match selected Cargo profile'
        printf 'copy:%s\n' "$1"
    }
    # macOS Bash 3.2 treats an empty array as unset under nounset; this mock
    # checks argv/copy wiring, not the Linux Bash version prerequisite.
    output=$(set +u; eval "$(sed -n '/^cargo build --locked .*--example application-source$/,/^observer_args=()/p' "$fixture/run-application-retracking.sh" | sed '$d')")
    [[ $(printf '%s\n' "$output" | awk '/^cargo:/ {n++} END {print n+0}') == 2 &&
       $(printf '%s\n' "$output" | awk '/^copy:/ {n++} END {print n+0}') == 2 ]] ||
        fail 'both fixtures must use the selected build and copy profile'
)
check_fixture_profile dev debug ''
check_fixture_profile release release '--release '

check_driver_dispatch() (
    test_name=$1
    listing=$2
    test_profile_args=(--release)
    lab="$workspace/target/native-tests/driver-dispatch-test"
    cargo() {
        [[ "$*" == 'test --locked --features native-lab --release -p mixrelay-linux-driver --lib -- --list' ]] ||
            fail 'native Driver listing dispatched to the wrong Cargo target'
        [[ -z "$listing" ]] || printf '%s: test\n' "$listing"
    }
    eval "$(sed -n '/^elif .*native_driver_/,/^elif .*native_linux_service_/p' "$fixture/run-application-retracking.sh" | sed '1d;$d')"
    [[ "${test_command[*]}" == "env MIXRELAY_LAB_APPLICATION_FIXTURE=$lab/application-fixture cargo test --locked --features native-lab --release -p mixrelay-linux-driver --lib $listing -- --exact --nocapture --test-threads=1" ]] ||
        fail 'native Driver command lost exact native test/profile/fixture selection'
)
for driver_test in native_driver_32_64_exact_formats_and_explicit_removal \
    native_driver_32_64_pcm_continues_after_graph_client_exit \
    native_driver_same_device_capture_graph_render_has_causal_pcm; do
    if [[ "$driver_test" == native_driver_32_64_exact_formats_and_explicit_removal ]]; then
        check_driver_dispatch "$driver_test" "tests::$driver_test"
    else
        check_driver_dispatch "$driver_test" "tests::native_pcm::$driver_test"
    fi
done
if check_driver_dispatch native_driver_missing ''; then
    fail 'missing native Driver test could silently run zero tests'
fi

check_private_hook() (
    requested_hook=$1
    policy_lab="$workspace/target/native-tests/app-lab-hook-$requested_hook"
    mkdir -p "$policy_lab/config" "$policy_lab/data"
    wireplumber() { printf 'WirePlumber 0.5.8\n'; }
    cp() {
        if [[ "$1" == -R && "$2" == /usr/share/wireplumber ]]; then
            mkdir -p "$3/scripts"
            printf 'standard-script-sentinel\n' >"$3/scripts/standard.lua"
        else
            command cp "$@"
        fi
    }
    source "$fixture/configure-application-policy.sh" "$fixture" "$policy_lab" "$requested_hook"
    if [[ "$requested_hook" == source-mute ]]; then
        cmp "$fixture/../../wireplumber/mixrelay-source-mute.lua" "$policy_lab/data/wireplumber/scripts/mixrelay-source-mute.lua" || fail 'private Lua differs from frozen source'
        cmp "$fixture/../../wireplumber/90-mixrelay-source-mute.conf" "$policy_lab/config/wireplumber/wireplumber.conf.d/90-mixrelay-source-mute.conf" || fail 'private fragment differs from frozen source'
        grep -Fxq 'metadata = ImplMetadata ("mixrelay-source-mute")' "$policy_lab/data/wireplumber/scripts/mixrelay-source-mute.lua" || fail 'hook metadata lacks a script-lifetime owner'
        awk '
            index($0, "  }:register ()") { registered = NR; registered_count++ }
            index($0, "  metadata:set (0, \"hook.version\", nil, \"1\")") { ready = NR; ready_count++ }
            index($0, "  Script:finish_activation ()") { finished = NR; finished_count++ }
            END {
                one_each = registered_count == 1 && ready_count == 1 && finished_count == 1
                ordered = registered < ready && ready < finished
                exit !(one_each && ordered)
            }
        ' "$policy_lab/data/wireplumber/scripts/mixrelay-source-mute.lua" || fail 'hook readiness must follow registration and precede activation finish'
        [[ $(<"$policy_lab/data/wireplumber/scripts/standard.lua") == standard-script-sentinel ]] || fail 'private data lookup lost standard scripts'
        [[ -s "$policy_lab/source-mute-hook.sha256" ]] || fail 'hook source and destination hashes were not recorded'
    else
        [[ ! -e "$policy_lab/data/wireplumber" && ! -e "$policy_lab/source-mute-hook.sha256" ]] || fail 'ordinary tests unexpectedly injected the Forward hook'
    fi
)
check_private_hook none
check_private_hook source-mute
unsupported_lab="$workspace/target/native-tests/app-lab-hook-unsupported"
mkdir -p "$unsupported_lab/config"
if (
    wireplumber() { printf 'WirePlumber 0.4.17\n'; }
    source "$fixture/configure-application-policy.sh" "$fixture" "$unsupported_lab" source-mute
); then
    fail 'Forward hook accepted unsupported WirePlumber 0.4'
fi
[[ ! -e "$unsupported_lab/config/wireplumber" ]] || fail 'unsupported Forward setup changed its private configuration'

# These globals are consumed by the sourced runner cleanup function.
# shellcheck disable=SC2034
lab="$workspace/target/native-tests/cache-preservation-test"
# shellcheck disable=SC2034
inventory_pid=''
# shellcheck disable=SC2034
profiler_pid=''
# shellcheck disable=SC2034
policy_pid=''
# shellcheck disable=SC2034
pipewire_pid=''
# shellcheck disable=SC2034
bus_pid=''
# shellcheck disable=SC2034
test_launcher_pid=''
cleanup
[[ "$(<"$cache/cache-sentinel")" == preserve-me ]] || fail 'runner cleanup removed or changed the Cargo cache'

printf 'PASS: Cargo target default, safe override, rejected paths, cache preservation, and artifact wiring\n'
