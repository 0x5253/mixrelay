import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  cpSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

// These tests run the macOS zsh build script; Linux lab images have no zsh or Apple tools.
const zshTest = (name, fn) => test(name, { skip: process.platform !== "darwin" && "macOS zsh build script" }, fn);

const sourceScript = new URL("./build-macos-universal-engine.sh", import.meta.url);

function writeExecutable(path, contents) {
  writeFileSync(path, contents, { mode: 0o755 });
  chmodSync(path, 0o755);
}

function installFakeCommands(bin) {
  writeExecutable(join(bin, "uname"), `#!/bin/sh
if [ "$1" = "-s" ]; then
  printf '%s\\n' Darwin
else
  /usr/bin/uname "$@"
fi
`);
  writeExecutable(join(bin, "rustup"), `#!/bin/sh
if [ "$1" = "target" ] && [ "$2" = "list" ] && [ "$3" = "--installed" ]; then
  printf '%s\\n' aarch64-apple-darwin x86_64-apple-darwin
  exit 0
fi
exit 2
`);
  writeExecutable(join(bin, "cargo"), `#!/bin/sh
set -eu
target=""
target_dir=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --target|-p|--target-dir)
      if [ "$1" = "--target-dir" ]; then
        target_dir="$2"
      elif [ "$1" = "--target" ]; then
        target="$2"
      fi
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '%s|%s|%s\\n' "$target" "$target_dir" "\${CARGO_BUILD_JOBS:-}" >> "$FAKE_CARGO_LOG"
if [ "\${FAKE_CARGO_FAIL_TARGET:-}" = "$target" ]; then
  exit 31
fi
if [ "\${FAKE_CARGO_MISSING_TARGET:-}" = "$target" ]; then
  exit 0
fi
output="$target_dir/$target/release/mixrelay-service"
mkdir -p "$(dirname "$output")"
printf 'slice=%s\\n' "$target" > "$output"
chmod 755 "$output"
`);
  writeExecutable(join(bin, "xcrun"), `#!/bin/sh
set -eu
if [ "$#" -ge 3 ] && [ "$1" = "--sdk" ] && [ "$3" = "--show-sdk-path" ]; then
  printf '%s\\n' "$FAKE_SDK_PATH"
  exit 0
fi
if [ "$#" -ge 4 ] && [ "$1" = "--sdk" ] && [ "$3" = "--find" ] && [ "$4" = "clang" ]; then
  printf '%s\\n' /usr/bin/clang
  exit 0
fi
if [ "$1" != "lipo" ]; then
  exit 2
fi
shift
if [ "$1" = "-create" ]; then
  shift
  output=""
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "-output" ]; then
      output="$2"
      shift 2
    else
      shift
    fi
  done
  if [ "\${FAKE_LIPO_FAIL:-}" = "1" ]; then
    exit 41
  fi
  mkdir -p "$(dirname "$output")"
  printf 'archs=%s\\n' "\${FAKE_LIPO_ARCHS:-arm64 x86_64}" > "$output"
  exit 0
fi
if [ "$1" = "-archs" ]; then
  sed -n 's/^archs=//p' "$2"
  exit 0
fi
exit 2
`);
  writeExecutable(join(bin, "codesign"), `#!/bin/sh
set -eu
printf '%s\\n' "$*" >> "$FAKE_CODESIGN_LOG"
if [ "\${FAKE_CODESIGN_FAIL:-}" = "1" ] && [ "$1" = "--force" ]; then
  exit 43
fi
`);
}

function fixture(t) {
  const root = mkdtempSync(join(realpathSync(tmpdir()), "mixrelay-universal-engine-"));
  const scripts = join(root, "scripts");
  const bin = join(root, "bin");
  mkdirSync(scripts);
  mkdirSync(bin);
  const script = join(scripts, "build-macos-universal-engine.sh");
  cpSync(sourceScript, script);
  copyFileSync(new URL("./macos-minimum.mjs", import.meta.url), join(root, "scripts/macos-minimum.mjs"));
  mkdirSync(join(root, "support"), { recursive: true });
  copyFileSync(new URL("../support/environments.json", import.meta.url), join(root, "support/environments.json"));
  mkdirSync(join(root, "apps/desktop-shell/src-tauri"), { recursive: true });
  copyFileSync(new URL("../apps/desktop-shell/src-tauri/tauri.conf.json", import.meta.url), join(root, "apps/desktop-shell/src-tauri/tauri.conf.json"));
  chmodSync(script, 0o755);
  const entitlements = join(root, "native/macos/service/Engine.entitlements.plist");
  mkdirSync(join(root, "native/macos/service"), { recursive: true });
  copyFileSync(new URL("../native/macos/service/Engine.entitlements.plist", import.meta.url), entitlements);
  copyFileSync(new URL("../native/macos/service/Info.plist", import.meta.url), join(root, "native/macos/service/Info.plist"));
  installFakeCommands(bin);

  const logs = {
    cargo: join(root, "cargo.log"),
    codesign: join(root, "codesign.log"),
  };
  writeFileSync(logs.cargo, "");
  writeFileSync(logs.codesign, "");
  const environment = {
    ...process.env,
    PATH: `${bin}:${process.env.PATH}`,
    DEVELOPER_DIR: root,
    CARGO_TARGET_DIR: "",
    CARGO_BUILD_JOBS: "",
    MIXRELAY_CODESIGN_IDENTITY: "-",
    FAKE_CARGO_FAIL_TARGET: "",
    FAKE_CARGO_MISSING_TARGET: "",
    FAKE_LIPO_FAIL: "",
    FAKE_LIPO_ARCHS: "arm64 x86_64",
    FAKE_CODESIGN_FAIL: "",
    FAKE_CARGO_LOG: logs.cargo,
    FAKE_CODESIGN_LOG: logs.codesign,
    FAKE_SDK_PATH: join(root, "sdk"),
  };
  t.after(() => rmSync(root, { force: true, recursive: true }));
  return { environment, logs, root, script };
}

function runScript(paths, args = [], overrides = {}) {
  return spawnSync("/bin/zsh", [paths.script, ...args], {
    cwd: paths.root,
    encoding: "utf8",
    env: { ...paths.environment, ...overrides },
  });
}

function universalBinary(paths, targetDir = "target") {
  return join(paths.root, targetDir, "universal-apple-darwin", "release", "mixrelay-service");
}

function universalRelease(paths, targetDir = "target") {
  return join(paths.root, targetDir, "universal-apple-darwin", "release");
}

function cargoCalls(paths) {
  return readFileSync(paths.logs.cargo, "utf8").trim().split("\n").filter(Boolean).map(line => line.split("|"));
}

function seedPreviousBinary(paths, targetDir = "target") {
  const release = universalRelease(paths, targetDir);
  mkdirSync(release, { recursive: true });
  const output = universalBinary(paths, targetDir);
  writeFileSync(output, "previous-universal-engine");
  chmodSync(output, 0o755);
  writeFileSync(join(release, ".unrelated-temp"), "preserve-me");
  return output;
}

function assertFailurePreserved(paths, previous, targetDir = "target") {
  assert.equal(readFileSync(previous, "utf8"), "previous-universal-engine");
  assert.equal(lstatSync(previous).isFile(), true);
  assert.equal(existsSync(join(universalRelease(paths, targetDir), ".unrelated-temp")), true);
  const leftovers = readdirSync(universalRelease(paths, targetDir))
    .filter(name => name.startsWith(".mixrelay-service-universal."));
  assert.deepEqual(leftovers, []);
}

zshTest("builds both slices, signs the lipo output, and cleans its temporary directory", t => {
  const paths = fixture(t);
  seedPreviousBinary(paths);
  const result = runScript(paths);
  assert.equal(result.status, 0, result.stderr);

  const output = universalBinary(paths);
  assert.equal(readFileSync(output, "utf8"), "archs=arm64 x86_64\n");
  assert.equal(lstatSync(output).mode & 0o111, 0o111);
  assert.deepEqual(cargoCalls(paths), [
    // No job limit by default: cargo builds with every core.
    ["aarch64-apple-darwin", join(paths.root, "target"), ""],
    ["x86_64-apple-darwin", join(paths.root, "target"), ""],
  ]);
  const codesignCalls = readFileSync(paths.logs.codesign, "utf8").trim().split("\n").filter(Boolean);
  assert.equal(codesignCalls.length, 3);
  assert.match(codesignCalls[0], /^--force --sign - /);
  assert.doesNotMatch(codesignCalls[0], /--options|--timestamp|--identifier|--entitlements/);
  assert.deepEqual(readdirSync(universalRelease(paths)).sort(), [".unrelated-temp", "mixrelay-service"]);
});

zshTest("release signing uses the exact runtime policy, identifier, and engine entitlements", t => {
  const paths = fixture(t);
  // The script signs with whatever identity the embedded Info.plist declares.
  const infoPlist = join(paths.root, "native/macos/service/Info.plist");
  writeFileSync(infoPlist, readFileSync(infoPlist, "utf8").replace(/(<key>CFBundleIdentifier<\/key>\s*<string>)[^<]+/, "$1io.mixrelay.fixture-engine"));
  const identity = "Developer ID Application: MixRelay, Inc. (ABCDEF1234)";
  const result = runScript(paths, [], { MIXRELAY_CODESIGN_IDENTITY: identity });
  assert.equal(result.status, 0, result.stderr);

  const calls = readFileSync(paths.logs.codesign, "utf8").trim().split("\n").filter(Boolean);
  assert.equal(calls.length, 3);
  const temporaryBinary = calls[0].split(" ").at(-1);
  assert.equal(
    calls[0],
    `--force --sign ${identity} --options runtime --timestamp --identifier io.mixrelay.fixture-engine --entitlements ${join(paths.root, "native/macos/service/Engine.entitlements.plist")} ${temporaryBinary}`,
  );
  assert.match(calls[0], /--options runtime --timestamp --identifier io\.mixrelay\.fixture-engine --entitlements /);
  assert.match(calls[0], /\/\.mixrelay-service-universal\.[^/]+\/mixrelay-service$/);
  const plist = readFileSync(join(paths.root, "native/macos/service/Engine.entitlements.plist"), "utf8");
  assert.match(plist, /<key>com\.apple\.security\.device\.audio-input<\/key>\s*<true\/>/);
  assert.match(plist, /<key>com\.apple\.security\.cs\.disable-library-validation<\/key>\s*<true\/>/);
  assert.equal((plist.match(/<key>/g) ?? []).length, 2);
});

zshTest("rejects malformed and non-Developer-ID signing identities before invoking build tools", t => {
  for (const identity of [
    "",
    "Apple Development: MixRelay (ABCDEF1234)",
    "Developer ID Application: MixRelay",
    "Developer ID Application:  MixRelay (ABCDEF1234)",
    "Developer ID Application: MixRelay  (ABCDEF1234)",
    "Developer ID Application: MixRelay\t(ABCDEF1234)",
    "Developer ID Installer: MixRelay (ABCDEF1234)",
  ]) {
    const paths = fixture(t);
    const previous = seedPreviousBinary(paths);
    const result = runScript(paths, [], { MIXRELAY_CODESIGN_IDENTITY: identity });
    assert.notEqual(result.status, 0, JSON.stringify(identity));
    assert.match(result.stderr, /MIXRELAY_CODESIGN_IDENTITY|Release signing identity/);
    assert.equal(readFileSync(paths.logs.cargo, "utf8"), "");
    assert.equal(readFileSync(paths.logs.codesign, "utf8"), "");
    assertFailurePreserved(paths, previous);
  }
});

zshTest("--prebuilt merges and signs existing slices without running cargo", t => {
  const paths = fixture(t);
  const built = runScript(paths);
  assert.equal(built.status, 0, built.stderr);
  writeFileSync(paths.logs.cargo, "");
  const result = runScript(paths, ["--prebuilt"]);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(cargoCalls(paths), []);
  assert.equal(existsSync(universalBinary(paths)), true);
  assert.notEqual(runScript(paths, ["--prebuilt", "extra"]).status, 0);
});

zshTest("honors CARGO_TARGET_DIR and an explicit CARGO_BUILD_JOBS override", t => {
  const paths = fixture(t);
  const result = runScript(paths, [], { CARGO_BUILD_JOBS: "3", CARGO_TARGET_DIR: "custom-target" });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(existsSync(universalBinary(paths, "custom-target")), true);
  assert.deepEqual(cargoCalls(paths), [
    ["aarch64-apple-darwin", join(paths.root, "custom-target"), "3"],
    ["x86_64-apple-darwin", join(paths.root, "custom-target"), "3"],
  ]);
  assert.equal(existsSync(join(paths.root, "target")), false);
});

zshTest("cargo, lipo, and signing failures preserve the previous binary and clean only owned temp", t => {
  const scenarios = [
    { name: "cargo", overrides: { FAKE_CARGO_FAIL_TARGET: "x86_64-apple-darwin" } },
    { name: "lipo", overrides: { FAKE_LIPO_FAIL: "1" } },
    { name: "codesign", overrides: { FAKE_CODESIGN_FAIL: "1" } },
    {
      name: "release codesign",
      overrides: {
        FAKE_CODESIGN_FAIL: "1",
        MIXRELAY_CODESIGN_IDENTITY: "Developer ID Application: MixRelay, Inc. (ABCDEF1234)",
      },
    },
  ];
  for (const scenario of scenarios) {
    const paths = fixture(t);
    const previous = seedPreviousBinary(paths);
    const result = runScript(paths, [], scenario.overrides);
    assert.notEqual(result.status, 0, `${scenario.name} unexpectedly succeeded`);
    assertFailurePreserved(paths, previous);
  }
});

zshTest("rejects missing or non-exact architecture slices without publishing", t => {
  const missing = fixture(t);
  const missingPrevious = seedPreviousBinary(missing);
  const missingResult = runScript(missing, [], { FAKE_CARGO_MISSING_TARGET: "x86_64-apple-darwin" });
  assert.notEqual(missingResult.status, 0);
  assertFailurePreserved(missing, missingPrevious);

  const wrong = fixture(t);
  const wrongPrevious = seedPreviousBinary(wrong);
  const wrongResult = runScript(wrong, [], { FAKE_LIPO_ARCHS: "arm64 arm64" });
  assert.notEqual(wrongResult.status, 0);
  assertFailurePreserved(wrong, wrongPrevious);
});

zshTest("rejects an existing destination directory or symlink before publishing", t => {
  const directory = fixture(t);
  const directoryOutput = universalBinary(directory);
  mkdirSync(directoryOutput, { recursive: true });
  writeFileSync(join(directoryOutput, "stale"), "stale");
  const directoryResult = runScript(directory);
  assert.notEqual(directoryResult.status, 0);
  assert.equal(lstatSync(directoryOutput).isDirectory(), true);
  assert.equal(existsSync(join(directoryOutput, "stale")), true);

  const symlink = fixture(t);
  const symlinkPrevious = join(symlink.root, "previous-engine");
  writeFileSync(symlinkPrevious, "previous");
  const symlinkOutput = universalBinary(symlink);
  mkdirSync(universalRelease(symlink), { recursive: true });
  symlinkSync(symlinkPrevious, symlinkOutput);
  const symlinkResult = runScript(symlink);
  assert.notEqual(symlinkResult.status, 0);
  assert.equal(lstatSync(symlinkOutput).isSymbolicLink(), true);
  assert.equal(readFileSync(symlinkPrevious, "utf8"), "previous");
});

zshTest("rejects unsupported arguments without invoking build tools", t => {
  const paths = fixture(t);
  const result = runScript(paths, ["--target", "aarch64-apple-darwin"]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /usage:/);
  assert.equal(readFileSync(paths.logs.cargo, "utf8"), "");
  assert.equal(existsSync(join(paths.root, "target")), false);
});
