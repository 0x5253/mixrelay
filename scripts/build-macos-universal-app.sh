#!/bin/zsh
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  print -u2 "MixRelay's Universal app can only be built on macOS"
  exit 1
fi

repo_root="${0:A:h:h}"
minimum_macos="$(node "$repo_root/scripts/macos-minimum.mjs")"

if (( $# != 0 )); then
  print -u2 "Usage: scripts/build-macos-universal-app.sh"
  exit 1
fi
if [[ -n "${MIXRELAY_CODESIGN_IDENTITY:-}" && -n "${APPLE_SIGNING_IDENTITY:-}" &&
      "${MIXRELAY_CODESIGN_IDENTITY}" != "${APPLE_SIGNING_IDENTITY}" ]]; then
  print -u2 "MIXRELAY_CODESIGN_IDENTITY conflicts with APPLE_SIGNING_IDENTITY"
  exit 1
fi
# Child tools read different names: Tauri reads APPLE_SIGNING_IDENTITY and the engine
# builder reads MIXRELAY_CODESIGN_IDENTITY (ad-hoc '-' when unset). Export both.
if [[ -n "${MIXRELAY_CODESIGN_IDENTITY:-}" ]]; then
  export APPLE_SIGNING_IDENTITY="$MIXRELAY_CODESIGN_IDENTITY"
fi
if [[ "${APPLE_SIGNING_IDENTITY:-}" != "Developer ID Application: "* ]]; then
  print -u2 "A Developer ID Application signing identity is required for a release bundle"
  exit 1
fi
if true; then
  if [[ ! "$APPLE_SIGNING_IDENTITY" =~ '\(([A-Z0-9]{10})\)$' ]]; then
    print -u2 "Driver signing identity must contain its ten-character Apple Team ID"
    exit 1
  fi
  signing_team="${match[1]}"
  export MIXRELAY_CODESIGN_IDENTITY="$APPLE_SIGNING_IDENTITY"

fi
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  print -u2 "Unset CARGO_TARGET_DIR for the checked universal app build"
  exit 1
fi
# Cargo uses every core unless the caller sets an explicit job count.
if [[ -z "${CARGO_BUILD_JOBS:-}" ]]; then
  unset CARGO_BUILD_JOBS
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

app_bundle="$repo_root/target/universal-apple-darwin/release/bundle/macos/MixRelay.app"
if true; then
  app_bundle="$repo_root/target/universal-apple-darwin/release/bundle/macos/MixRelay.app"
fi
bundle_parent="${app_bundle:h}"
backup_root=""
backup_bundle=""
old_bundle_moved=0
build_started=0

reject_symlink_components() {
  local path="$1"
  local current="/"
  local component
  local -a components
  components=("${(@s:/:)path}")
  for component in "${components[@]}"; do
    [[ -z "$component" ]] && continue
    current="${current%/}/$component"
    if [[ -L "$current" ]]; then
      print -u2 "Refusing symlink path component: $current"
      return 1
    fi
    [[ -e "$current" ]] || break
  done
}

cleanup() {
  local exit_code=$?
  local cleanup_failed=0
  trap - EXIT
  set +e

  if (( exit_code == 0 )); then
    if [[ -n "$backup_root" && -e "$backup_root" ]]; then
      if ! rm -rf "$backup_root"; then
        print -u2 "Could not remove the release backup; preserving it at $backup_root"
        cleanup_failed=1
      fi
    fi
    (( cleanup_failed == 0 )) || exit_code=1
    exit "$exit_code"
  fi

  if (( old_bundle_moved )); then
    if [[ -e "$app_bundle" || -L "$app_bundle" ]]; then
      rm -rf "$app_bundle"
    fi
    if [[ -e "$app_bundle" || -L "$app_bundle" ]]; then
      print -u2 "Could not remove the failed new app output; preserving recovery backup at $backup_root"
      exit 1
    fi
    if [[ -e "$backup_bundle" || -L "$backup_bundle" ]]; then
      if ! mv "$backup_bundle" "$app_bundle"; then
        print -u2 "Could not restore the previous app; preserving recovery backup at $backup_root"
        exit 1
      fi
    else
      print -u2 "Previous app backup is missing; preserving recovery directory at $backup_root"
      exit 1
    fi
    if [[ -e "$backup_root" ]] && ! rm -rf "$backup_root"; then
      print -u2 "Could not remove the restored app backup directory; preserving it at $backup_root"
      exit 1
    fi
  else
    if [[ -n "$backup_root" && -e "$backup_root" ]]; then
      rm -rf "$backup_root"
    fi
    if (( build_started )) && [[ -e "$app_bundle" || -L "$app_bundle" ]]; then
      rm -rf "$app_bundle"
    fi
  fi
  exit "$exit_code"
}

trap cleanup EXIT
reject_symlink_components "$bundle_parent"
if [[ -L "$app_bundle" ]]; then
  print -u2 "Expected app bundle must not be a symlink: $app_bundle"
  exit 1
fi
if [[ -e "$app_bundle" || -d "$app_bundle" ]]; then
  if [[ ! -d "$app_bundle" ]]; then
    print -u2 "Expected app bundle must be a directory: $app_bundle"
    exit 1
  fi
  if [[ -n "$(find "$app_bundle" -type l -print -quit)" ]]; then
    print -u2 "Expected app bundle must not contain symlinks: $app_bundle"
    exit 1
  fi
  backup_root="$(mktemp -d "$bundle_parent/.mixrelay-universal-app-backup.XXXXXX")"
  backup_bundle="$backup_root/${app_bundle:t}"
  if ! mv "$app_bundle" "$backup_bundle"; then
    print -u2 "Could not move the previous app bundle to its private backup"
    exit 1
  fi
  old_bundle_moved=1
fi

cd "$repo_root"
build_started=1

# Each step reports its duration so slow phases are visible in the release log.
step_name=""
step_started=0
step() {
  finish_step
  step_name="$1"
  step_started=$SECONDS
  print "==> $step_name"
}
finish_step() {
  if [[ -n "$step_name" ]]; then
    print "==> $step_name: $((SECONDS - step_started))s"
  fi
  step_name=""
}

if true; then
  # The editor UI and icons are compiled into the desktop binary, so they come first.
  step "Editor UI and icons"
  pnpm desktop:icons
  pnpm --dir apps/control-ui build

  # One cargo build per architecture compiles every release binary together, so
  # shared crates are compiled once per architecture instead of once per product.
  # Later steps only merge architectures, sign and bundle; they do not run cargo.
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    step "Compile release binaries ($target)"
    cargo build --locked --release --target "$target" \
      -p mixrelay-macos-installer \
      -p mixrelay-service \
      -p mixrelay-driver-service \
      -p mixrelay-desktop \
      --bins --features mixrelay-desktop/custom-protocol
  done

  step "Universal helpers"
  helper_output="$repo_root/target/universal-apple-darwin/release"
  mkdir -p "$helper_output"
  for helper in mixrelay-macos-installer mixrelay-macos-payload-check mixrelay-driver-uninstaller; do
    reject_symlink_components "$helper_output/$helper"
    xcrun lipo -create \
      "$repo_root/target/aarch64-apple-darwin/release/$helper" \
      "$repo_root/target/x86_64-apple-darwin/release/$helper" \
      -output "$helper_output/$helper"
    architectures="$(xcrun lipo -archs "$helper_output/$helper")"
    if [[ "$architectures" != "arm64 x86_64" && "$architectures" != "x86_64 arm64" ]]; then
      print -u2 "Driver helper must contain exactly arm64 and x86_64: $helper ($architectures)"
      exit 1
    fi
    codesign --force --sign "$APPLE_SIGNING_IDENTITY" --options runtime \
      --timestamp --identifier "$helper" "$helper_output/$helper"
    codesign --verify --strict --all-architectures "$helper_output/$helper"
  done
  # One bundle supplies both the editor and its internal background role.
  step "Universal launcher, engine and Driver service"
  zsh "$repo_root/scripts/build-macos-driver-launcher.sh"
  zsh "$repo_root/scripts/build-macos-universal-engine.sh" --prebuilt
  pnpm desktop:prepare-driver --profile release --prebuilt

  step "Universal app bundle"
  desktop_universal="$repo_root/target/universal-apple-darwin/release/mixrelay-desktop"
  reject_symlink_components "$desktop_universal"
  xcrun lipo -create \
    "$repo_root/target/aarch64-apple-darwin/release/mixrelay-desktop" \
    "$repo_root/target/x86_64-apple-darwin/release/mixrelay-desktop" \
    -output "$desktop_universal"
  pnpm --filter @mixrelay/desktop tauri bundle \
    --bundles app --target universal-apple-darwin --features custom-protocol
  finish_step
fi

if [[ -L "$app_bundle" || ! -d "$app_bundle" ]]; then
  print -u2 "Expected app bundle was not produced: $app_bundle"
  exit 1
fi
if [[ -n "$(find "$app_bundle" -type l -print -quit)" ]]; then
  print -u2 "Produced app bundle must not contain symlinks: $app_bundle"
  exit 1
fi


mach_o_count=0
while IFS= read -r -d '' candidate; do
  file_description="$(file -b "$candidate")"
  if [[ "$file_description" != *"Mach-O"* ]]; then
    continue
  fi

  architectures="$(xcrun lipo -archs "$candidate")"
  architecture_count="$(print -r -- "$architectures" | wc -w | tr -d ' ')"
  if [[ "$architecture_count" != "2" || " $architectures " != *" arm64 "* || " $architectures " != *" x86_64 "* ]]; then
    print -u2 "Every bundled Mach-O must contain exactly arm64 and x86_64"
    print -u2 "Invalid binary: $candidate ($architectures)"
    exit 1
  fi
  mach_o_count=$((mach_o_count + 1))
  print "Universal Mach-O: ${candidate#$app_bundle/} ($architectures)"
done < <(find "$app_bundle" -type f -print0)

if (( mach_o_count == 0 )); then
  print -u2 "No Mach-O executable was found in $app_bundle"
  exit 1
fi

if ! codesign --verify --deep --strict "$app_bundle" 2>/dev/null; then
  print -u2 "Universal app is not fully signed; Tauri signing must produce a valid nested signature"
  exit 1
fi

print "MixRelay Universal app: $app_bundle"

if true; then
  remover_bundle="$bundle_parent/Uninstall MixRelay.app"
  reject_symlink_components "$remover_bundle"
  version="$(plutil -extract CFBundleShortVersionString raw -o - "$app_bundle/Contents/Info.plist")"
  node scripts/build-macos-removal-app.mjs \
    "$helper_output/mixrelay-driver-uninstaller" "$helper_output/mixrelay-macos-installer" \
    "$remover_bundle" "$version" "$APPLE_SIGNING_IDENTITY"
fi
