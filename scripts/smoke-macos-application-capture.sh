#!/bin/zsh
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  print -u2 "macOS application capture smoke requires macOS"
  exit 1
fi

repo_root="${0:A:h:h}"
minimum_macos="$(node "$repo_root/scripts/macos-minimum.mjs")"
native_root="$repo_root/native/macos/application-capture"
endpoint_uid=""
registration_api=${MIXRELAY_LAB_TAP_REGISTRATION:-function}
if [[ "$registration_api" != function && "$registration_api" != block ]]; then
  print -u2 "MIXRELAY_LAB_TAP_REGISTRATION must be function or block"
  exit 2
fi
if (( $# > 1 )); then
  print -u2 "usage: $0 [exact-owned-endpoint-uid-or-canonical-uuid]"
  exit 1
fi
if (( $# == 1 )); then
  selected="$1"
  uuid_pattern='^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
  uid_pattern='^io\.mixrelay\.audio\.endpoint\.[0-9a-f]{32}$'
  if [[ "$selected" =~ "$uuid_pattern" ]]; then
    endpoint_uid="io.mixrelay.audio.endpoint.${selected//-/}"
  elif [[ "$selected" =~ "$uid_pattern" ]]; then
    endpoint_uid="$selected"
  else
    print -u2 "explicit canonical MixRelay endpoint required; refusing default fallback"
    exit 1
  fi
  if [[ "$endpoint_uid" == "io.mixrelay.audio.endpoint.00000000000000000000000000000000" ]]; then
    print -u2 "zero endpoint identity is not owned"
    exit 1
  fi
fi
cargo build --manifest-path "$repo_root/Cargo.toml" -p mixrelay-process-tap --release
capture_archive="$repo_root/target/release/libmixrelay_process_tap.a"
smoke_build_dir="$repo_root/target/native-tests/application-capture"
mkdir -p "$smoke_build_dir"
tone_process_id=""
cleanup() {
  if [[ -n "$tone_process_id" ]]; then
    if [[ -n "$endpoint_uid" ]]; then
      { print -p -- quit } 2>/dev/null || true
      for attempt in {1..50}; do
        kill -0 "$tone_process_id" 2>/dev/null || break
        sleep 0.02
      done
    fi
    kill "$tone_process_id" 2>/dev/null || true
    wait "$tone_process_id" 2>/dev/null || true
    tone_process_id=""
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
tone_name="MRToneSource"
tone_frameworks=(-framework AVFAudio -framework Foundation)
if [[ -n "$endpoint_uid" ]]; then
  tone_name="MRVirtualToneSource"
  tone_frameworks+=(-framework CoreAudio -framework AudioUnit)
fi
tone_bundle="$smoke_build_dir/$tone_name.app"
mkdir -p "$tone_bundle/Contents/MacOS"
cp "$native_root/tests/$tone_name-Info.plist" "$tone_bundle/Contents/Info.plist"

strict_flags=(
  "-mmacosx-version-min=$minimum_macos"
  -Wall
  -Wextra
  -Wpedantic
  -Wconversion
  -Wsign-conversion
  -Wshadow
  -Werror
)

clang \
  -fobjc-arc \
  -fmodules \
  -fblocks \
  "${strict_flags[@]}" \
  "$native_root/tests/$tone_name.m" \
  "${tone_frameworks[@]}" \
  -o "$tone_bundle/Contents/MacOS/$tone_name"

clang \
  -std=c17 \
  -fobjc-arc \
  -fmodules \
  "${strict_flags[@]}" \
  -I "$native_root/include" \
  "$native_root/tests/MRProcessTapSmoke.m" \
  "$capture_archive" -liconv \
  -framework CoreAudio \
  -framework AudioToolbox \
  -framework Foundation \
  -framework AppKit \
  -o "$smoke_build_dir/MRProcessTapSmoke"

if [[ "$registration_api" == block ]]; then
  clang -std=c17 -fblocks -dynamiclib "${strict_flags[@]}" \
    "$native_root/tests/MRIOProcBlockRegistration.m" -framework CoreAudio \
    -o "$smoke_build_dir/MRIOProcBlockRegistration.dylib"
fi

if [[ -n "$endpoint_uid" ]]; then
  coproc "$tone_bundle/Contents/MacOS/$tone_name" --mixrelay-virtual-only "$endpoint_uid"
  tone_process_id=$!
  readiness=""
  if ! read -r -p -t 5 readiness || [[ "$readiness" != "ready $endpoint_uid" ]]; then
    print -u2 "owned tone did not acknowledge the exact endpoint"
    exit 1
  fi
else
  "$tone_bundle/Contents/MacOS/$tone_name" &
  tone_process_id=$!
  sleep 1
fi
smoke_arguments=(io.mixrelay.test-tone-source)
if [[ -n "$endpoint_uid" ]]; then
  smoke_arguments=(io.mixrelay.test-virtual-clock-source "$endpoint_uid")
fi
set +e
# Native registration itself can block inside CoreAudio before the fixture's
# measurement loop starts. Bound the exact owned child, then let EXIT stop its tone.
node --input-type=module - "$smoke_build_dir/MRProcessTapSmoke" "$registration_api" \
  "$smoke_build_dir/MRIOProcBlockRegistration.dylib" "${smoke_arguments[@]}" <<'JS'
import { spawnSync } from 'node:child_process';
const environment = { ...process.env };
if (process.argv[3] === 'block') environment.DYLD_INSERT_LIBRARIES = process.argv[4];
console.error(`registration_api=${process.argv[3]} (owned native fixture)`);
const result = spawnSync(process.argv[2], process.argv.slice(5), {
  env: environment, stdio: 'inherit', timeout: 30000, killSignal: 'SIGKILL',
});
if (result.error) console.error(`Application-capture smoke failed: ${result.error.message}`);
process.exit(result.status ?? 1);
JS
smoke_status=$?
set -e
if [[ -z "$endpoint_uid" ]]; then
  set +e
  wait "$tone_process_id"
  tone_status=$?
  set -e
  tone_process_id=""
  if (( tone_status != 0 )); then
    exit "$tone_status"
  fi
fi
exit "$smoke_status"
