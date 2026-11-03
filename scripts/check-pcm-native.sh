#!/usr/bin/env bash
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
cargo build --manifest-path "$repo/Cargo.toml" -p mixrelay-pcm-ring --release --target-dir "$repo/target"
mkdir -p "$repo/target/native-tests"
test_dir=$(mktemp -d "$repo/target/native-tests/pcm.XXXXXX")
# This directory is created exclusively above and contains only this test's binary.
trap 'rm -f -- "$test_dir/pcm-tests"; rmdir -- "$test_dir"' EXIT
"${CC:-clang}" -std=c17 -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I "$repo/native/pcm-ring/include" \
  "$repo/native/pcm-ring/tests/MRPcmRingTests.c" \
  "$repo/target/release/libmixrelay_pcm_ring.a" -ldl -lm \
  -o "$test_dir/pcm-tests"
ASAN_OPTIONS=halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$test_dir/pcm-tests"
