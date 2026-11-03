# Temporary plugin-host ABI patches

`truce-rack-core`, `truce-rack-au`, `truce-rack-vst3`, `truce-rack-clap`, and
`truce-rack-lv2` are copied from the crates.io **1.1.5** packages.
Their recorded upstream commit is `233f56c7788291b2f24e3f2aba3a767330f72cb7` in
https://github.com/truce-audio/truce-rack. Root MIT and Apache licenses are preserved here.
Normalized Cargo manifests and upstream READMEs are retained. Registry caches are not modified.

The VST3/CLAP/LV2 manifests remove unused direct `thiserror` dependencies. These
three adapters use the error type supplied by `truce-rack-core`; they do not derive their own errors.
The upstream `.orig` manifests remain provenance, not active build inputs. This
dependency cleanup does not change plugin loading, DSP, native ABI or format support.

The core manifest also omits unused `crossbeam-queue`; AU omits unused `thiserror`.

An actual Debian 13/aarch64 workspace build exposed 9 VST3 and 2 CLAP type errors:
the bindings use target-dependent `c_char`, whereas these adapters hard-coded `i8`.
Use VST3 `char8`/`TUID`/`FIDString` and CLAP `c_char` without changing byte layout, DSP or ownership.
TUID and UTF-8 tests include bytes above 127 so signedness changes cannot silently corrupt IDs/names.

The root `[patch.crates-io]` applies this fix consistently on all platforms. These are excluded
from workspace membership so product lints do not accidentally rewrite upstream source.
Run their regression tests explicitly:

```sh
cargo test --locked -p truce-rack-vst3 -p truce-rack-clap --lib
```

Replace the patch with a released upstream fix only after both signed-c_char (macOS/x64) and
unsigned-c_char (Linux ARM64) builds and these tests pass. This is not a plugin runtime certification.

## LV2 state patch

The LV2 adapter's unimplemented state methods now call the standard LV2 State interface directly,
preserving native save/restore failures. Lilv's convenience state helpers otherwise discard native
save failures into a parameter-only snapshot and do not return native restore status. The format
stores a version, exact plugin URI, byte order, input control values by symbol, and owned opaque
properties keyed/typed by URI. `atom:URID` values are remapped across instances. State capture is
limited to portable POD properties (1 MiB total, 4096 entries) and a 64 MiB serialized envelope.
Host-private regular files referenced with `atom:Path` are packaged through LV2 State path callbacks,
with limits of 128 files and 40 MiB of aggregate asset bytes. These file-asset paths remain pending
the focused Linux/native fixture qualification below; this is not installed-vendor certification.

Nested URID containers, nonportable properties and a mismatched byte order remain explicit errors,
not silently discarded state. Restoration validates the complete envelope/control layout before
native mutation; the MixRelay host restores only a prepared worker and discards it on any vendor
restore failure.

The new `state.rs`, tests and `fixtures/state.lv2` fixture are MixRelay-authored under the same
MIT OR Apache-2.0 terms. Existing vendor licenses above remain applicable. Sources:
[LV2 state contract](https://lv2plug.in/ns/ext/state) and
[Lilv implementation](https://github.com/lv2/lilv/blob/master/src/state.c).

Linux fixture validation uses a temporary copy of `fixtures/state.lv2`, not installed plugins:

```sh
cc -shared -fPIC -Wall -Wextra -Werror "$fixture_dir/state.lv2/state.c" -o "$fixture_dir/state.lv2/state.so"
LV2_PATH="$fixture_dir" cargo test --locked -p truce-rack-lv2 --lib
LV2_PATH="$fixture_dir" cargo test --locked -p truce-rack-lv2 --lib native_state_fixture -- --ignored
```

On macOS the workspace intentionally does not enable LV2, so use the vendor manifest for a
standalone compile check; native linking/tests require Lilv. Do not label such a check as Linux
runtime verification.

## Intrinsic latency snapshot boundary

The local core adds `PluginCore::latency_frames() -> Option<u32>`: a cheap cached
intrinsic-frame snapshot, excluding MixRelay's worker transport. Known zero and
unknown/query failure remain distinct. No native getter is called by this method.
AUv3 uses the AU wrapper through the existing upstream delegate, so no duplicate
AUv3 implementation is introduced. Source lookup and cache updates stay inside
each format adapter, where native handle and lifecycle ownership already exist.

Native getter thread restrictions are not interchangeable: VST3 latency queries
belong in UI-thread setup, CLAP in main-thread activation/active lifecycle, and
AU in the serialized property/lifecycle path. LV2 instead reports a connected
output-control scalar; reading that scalar after `run` needs no native getter.
Format changes must invalidate/update snapshots rather than preserve a stale
known value. External LV2 `reportsLatency` interoperability does not retain any
superseded MixRelay implementation.

This patch is an adapter boundary, **not completed delay compensation**. Native
change notifications, control-thread restart/reconfiguration, worker-to-parent
publication, bounded parallel-route delay preparation, latency-preserving failure
bypass and dynamic PCM qualification must be integrated before closing that gate.
See the [current validation report](../docs/validation/2026-09-26-native-packets-and-packaging.md).
