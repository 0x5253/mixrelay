#!/bin/zsh
set -euo pipefail
repo_root="${0:A:h:h}"
minimum_macos="$(node "$repo_root/scripts/macos-minimum.mjs")"
[[ "${APPLE_SIGNING_IDENTITY:-}" == "Developer ID Application: "* ]] || {
  print -u2 "A Developer ID Application identity is required"
  exit 1
}
output_dir="$repo_root/target/universal-apple-darwin/release"
mkdir -p "$output_dir"
[[ ! -L "$output_dir" && ! -L "$output_dir/mixrelay-driver-launcher" ]] || exit 1
task_dir="$(mktemp -d "$output_dir/.driver-launcher.XXXXXX")"
trap 'rm -rf "$task_dir"' EXIT
for arch in arm64 x86_64; do
  xcrun --sdk macosx clang -arch "$arch" "-mmacosx-version-min=$minimum_macos" \
    -Wall -Wextra -Werror -O2 "$repo_root/native/macos/common/MRDriverRuntimeLauncher.c" \
    -o "$task_dir/$arch"
done
xcrun lipo -create "$task_dir/arm64" "$task_dir/x86_64" -output "$task_dir/mixrelay-driver-launcher"
codesign --force --sign "$APPLE_SIGNING_IDENTITY" --options runtime --timestamp \
  --identifier io.mixrelay.driver.launcher "$task_dir/mixrelay-driver-launcher"
codesign --verify --strict --all-architectures "$task_dir/mixrelay-driver-launcher"
mv "$task_dir/mixrelay-driver-launcher" "$output_dir/mixrelay-driver-launcher"
