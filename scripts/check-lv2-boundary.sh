#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} == --help ]]; then
  echo "Usage: bash scripts/check-lv2-boundary.sh"
  echo "Compile isolated repository LV2 fixtures and run every adapter unit test on Linux."
  exit 0
fi
[[ $# == 0 ]] || { echo "Unexpected arguments" >&2; exit 2; }
[[ $(uname -s) == Linux ]] || { echo "LV2 native boundary checks require Linux" >&2; exit 2; }
repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"
mkdir -p target/native-tests
fixtures=$(mktemp -d "$repo/target/native-tests/lv2.XXXXXX")
trap 'rm -rf -- "$fixtures"' EXIT
read -r -a lv2_flags <<< "$(pkg-config --cflags lv2)"
for name in multibus state; do
  source_dir="$repo/vendor/truce-rack-lv2/fixtures/$name.lv2"
  bundle="$fixtures/$name.lv2"
  mkdir "$bundle"
  cp "$source_dir/manifest.ttl" "$source_dir/$name.ttl" "$bundle/"
  "${CC:-cc}" -std=c17 -shared -fPIC -O2 -Wall -Wextra -Werror \
    "${lv2_flags[@]}" "$source_dir/$name.c" -o "$bundle/$name.so"
done
LV2_PATH="$fixtures" "${CARGO:-cargo}" test --locked \
  --manifest-path vendor/truce-rack-lv2/Cargo.toml --target-dir target \
  --config "patch.crates-io.truce-rack-core.path=\"$repo/vendor/truce-rack-core\"" \
  --lib -- --include-ignored
