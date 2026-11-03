#!/bin/zsh
set -euo pipefail

# --prebuilt merges and signs slices an earlier combined cargo build produced.
prebuilt=0
if (( $# == 1 )) && [[ "$1" == "--prebuilt" ]]; then
  prebuilt=1
elif (( $# != 0 )); then
  print -u2 "usage: $0 [--prebuilt]"
  exit 2
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  print -u2 "MixRelay's Universal engine can only be built on macOS"
  exit 1
fi

codesign_identity="${MIXRELAY_CODESIGN_IDENTITY--}"
if [[ -z "$codesign_identity" ]]; then
  print -u2 "MIXRELAY_CODESIGN_IDENTITY must be '-' for development or a Developer ID Application identity"
  exit 1
fi
if [[ "$codesign_identity" != "-" ]]; then
  if [[ ! "$codesign_identity" =~ '^Developer ID Application: ([^ ].*[^ ]|[^ ]) \([A-Z0-9]{10}\)$' || "$codesign_identity" == *[[:cntrl:]]* ]]; then
    print -u2 "Release signing identity must be a Developer ID Application identity with a 10-character Team ID"
    exit 1
  fi
fi

repo_root="${0:A:h:h}"
minimum_macos="$(node "$repo_root/scripts/macos-minimum.mjs")"
engine_entitlements="$repo_root/native/macos/service/Engine.entitlements.plist"
if [[ "$codesign_identity" != "-" && ! -f "$engine_entitlements" ]]; then
  print -u2 "Engine release entitlements are missing: $engine_entitlements"
  exit 1
fi
engine_info_plist="$repo_root/native/macos/service/Info.plist"
if [[ "$codesign_identity" != "-" ]]; then
  # Sign with the bundle identity the binary embeds, so the two cannot drift.
  engine_identifier="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$engine_info_plist")"
fi
target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
if [[ "$target_dir" != /* ]]; then
  target_dir="$repo_root/$target_dir"
fi

if [[ -z "${DEVELOPER_DIR:-}" ]]; then
  selected_developer_dir="$(xcode-select -p 2>/dev/null || true)"
  if [[ "$selected_developer_dir" == *".app/Contents/Developer" ]]; then
    export DEVELOPER_DIR="$selected_developer_dir"
  elif [[ -d /Applications/Xcode.app/Contents/Developer ]]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
  elif [[ -d /Applications/Xcode-26.6.0.app/Contents/Developer ]]; then
    export DEVELOPER_DIR=/Applications/Xcode-26.6.0.app/Contents/Developer
  else
    print -u2 "A full Xcode installation is required; set DEVELOPER_DIR explicitly"
    exit 1
  fi
fi

installed_targets="$(rustup target list --installed)"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  if [[ $'\n'"$installed_targets"$'\n' != *$'\n'"$target"$'\n'* ]]; then
    print -u2 "Missing Rust target $target; install it with: rustup target add $target"
    exit 1
  fi
done

export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
export MACOSX_DEPLOYMENT_TARGET="$minimum_macos"
compiler="$(xcrun --sdk macosx --find clang)"
export CC_aarch64_apple_darwin="$compiler"
export CC_x86_64_apple_darwin="$compiler"
export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$compiler"
export CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER="$compiler"

cd "$repo_root"
# Cargo uses every core unless the caller sets an explicit job count.
if [[ -z "${CARGO_BUILD_JOBS:-}" ]]; then
  unset CARGO_BUILD_JOBS
fi

if (( ! prebuilt )); then
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build \
      --locked \
      --release \
      -p mixrelay-service \
      --target "$target" \
      --target-dir "$target_dir"
  done
fi

universal_release_dir="$target_dir/universal-apple-darwin/release"
universal_binary="$universal_release_dir/mixrelay-service"
arm64_binary="$target_dir/aarch64-apple-darwin/release/mixrelay-service"
x86_64_binary="$target_dir/x86_64-apple-darwin/release/mixrelay-service"

if [[ -d "$universal_binary" || -L "$universal_binary" ]]; then
  print -u2 "Universal engine destination must be a regular file path: $universal_binary"
  exit 1
fi

for slice in "$arm64_binary" "$x86_64_binary"; do
  if [[ ! -f "$slice" ]]; then
    print -u2 "Expected engine slice was not produced: $slice"
    exit 1
  fi
done

mkdir -p "$universal_release_dir"
temporary_dir=""
cleanup() {
  if [[ -n "$temporary_dir" && -d "$temporary_dir" ]]; then
    rm -rf "$temporary_dir"
  fi
}
trap cleanup EXIT

temporary_dir="$(mktemp -d "$universal_release_dir/.mixrelay-service-universal.XXXXXX")"
temporary_binary="$temporary_dir/mixrelay-service"

xcrun lipo \
  -create \
  "$arm64_binary" \
  "$x86_64_binary" \
  -output "$temporary_binary"
chmod 755 "$temporary_binary"

verify_universal_architectures() {
  local binary="$1"
  local architectures
  local architecture
  local arm64_count=0
  local x86_64_count=0
  local architecture_count=0

  architectures="$(xcrun lipo -archs "$binary")"
  for architecture in ${=architectures}; do
    (( architecture_count += 1 ))
    case "$architecture" in
      arm64)
        (( arm64_count += 1 ))
        ;;
      x86_64)
        (( x86_64_count += 1 ))
        ;;
      *)
        print -u2 "Unexpected architecture in $binary: $architecture"
        return 1
        ;;
    esac
  done

  if (( architecture_count != 2 || arm64_count != 1 || x86_64_count != 1 )); then
    print -u2 "Expected exactly arm64 and x86_64 in $binary; found: $architectures"
    return 1
  fi
}

if ! verify_universal_architectures "$temporary_binary"; then
  exit 1
fi

if [[ "$codesign_identity" == "-" ]]; then
  codesign --force --sign "$codesign_identity" "$temporary_binary"
else
  codesign \
    --force \
    --sign "$codesign_identity" \
    --options runtime \
    --timestamp \
    --identifier "$engine_identifier" \
    --entitlements "$engine_entitlements" \
    "$temporary_binary"
fi
codesign --verify --strict "$temporary_binary"
if ! verify_universal_architectures "$temporary_binary"; then
  exit 1
fi

# The temporary file is in the destination directory, so this rename is atomic
# and cannot replace an existing binary until every build and verification above succeeds.
mv -f "$temporary_binary" "$universal_binary"

if [[ ! -x "$universal_binary" ]]; then
  print -u2 "Published engine is not executable: $universal_binary"
  exit 1
fi
codesign --verify --strict "$universal_binary"
if ! verify_universal_architectures "$universal_binary"; then
  exit 1
fi

print "MixRelay Universal engine: $universal_binary"
