import assert from "node:assert/strict";
import { chmodSync, copyFileSync, existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

// These tests run the macOS zsh build script; Linux lab images have no zsh or Apple tools.
const zshTest = (name, fn) => test(name, { skip: process.platform !== "darwin" && "macOS zsh build script" }, fn);

const sourceScript = fileURLToPath(new URL("./build-macos-universal-app.sh", import.meta.url));
const license = readFileSync(new URL("../LICENSE.txt", import.meta.url));

function executable(path, contents) {
  writeFileSync(path, contents);
  chmodSync(path, 0o755);
}

function createApp(path, { architecture = "universal", signed = false } = {}) {
  mkdirSync(join(path, "Contents", "MacOS"), { recursive: true });
  writeFileSync(join(path, "Contents", "MacOS", "MixRelay"), `MACHO ${architecture}\n`);
  if (signed) writeFileSync(join(path, ".tauri-signed"), "signed\n");
}

function fixture(t) {
  const fixtureRoot = fileURLToPath(new URL("../target/test-artifacts/", import.meta.url));
  mkdirSync(fixtureRoot, { recursive: true });
  const root = mkdtempSync(join(fixtureRoot, "mixrelay-universal-build-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const bin = join(root, "bin");
  const scripts = join(root, "scripts");
  const target = join(root, "target", "universal-apple-darwin", "release", "bundle", "macos");
  const app = join(target, "MixRelay.app");
  const unrelated = join(target, "unrelated.keep");
  const log = join(root, "mock-command.log");
  const sdk = join(root, "mock-sdk");
  const developer = join(root, "MockXcode.app", "Contents", "Developer");
  const script = join(scripts, "build-macos-universal-app.sh");
  mkdirSync(bin, { recursive: true });
  mkdirSync(scripts, { recursive: true });
  mkdirSync(target, { recursive: true });
  mkdirSync(sdk, { recursive: true });
  mkdirSync(developer, { recursive: true });
  mkdirSync(join(root, "apps/desktop/src-tauri/icons"), { recursive: true });
  writeFileSync(join(root, "apps/desktop/src-tauri/icons/icon.icns"), "icon fixture");
  writeFileSync(join(root, "LICENSE.txt"), license);
  writeFileSync(unrelated, "preserve me\n");
  copyFileSync(sourceScript, script);
  copyFileSync(new URL("./macos-minimum.mjs", import.meta.url), join(root, "scripts/macos-minimum.mjs"));
  mkdirSync(join(root, "support"), { recursive: true });
  copyFileSync(new URL("../support/environments.json", import.meta.url), join(root, "support/environments.json"));
  mkdirSync(join(root, "apps/desktop/src-tauri"), { recursive: true });
  copyFileSync(new URL("../apps/desktop/src-tauri/tauri.conf.json", import.meta.url), join(root, "apps/desktop/src-tauri/tauri.conf.json"));
  executable(join(scripts, "build-macos-driver-launcher.sh"), `#!/bin/sh
printf 'launcher built\\n' >> "$MOCK_LOG"
mkdir -p target/universal-apple-darwin/release
printf 'MACHO universal SIGNED\\n' > target/universal-apple-darwin/release/mixrelay-driver-launcher
`);
  executable(join(scripts, "build-macos-universal-engine.sh"), `#!/bin/sh
printf 'engine built %s\\n' "$*" >> "$MOCK_LOG"
printf 'engine identity %s\\n' "\${MIXRELAY_CODESIGN_IDENTITY-unset}" >> "$MOCK_LOG"
[ "\${MOCK_ENGINE_FAIL:-}" != 1 ]
`);
  writeFileSync(join(scripts,"build-macos-removal-app.mjs"), `import * as fs from 'node:fs'; import {join} from 'node:path'; const [, , executable,helper,output]=process.argv; fs.mkdirSync(join(output,'Contents'),{recursive:true}); fs.writeFileSync(join(output,'Contents/tested-arguments'),executable+'\\n'+helper);`);
  chmodSync(script, 0o755);

  executable(join(bin, "uname"), `#!/bin/sh
if [ "$1" = "-s" ]; then
  printf '%s\\n' Darwin
else
  printf '%s\\n' Darwin
fi
`);
  executable(join(bin, "rustup"), `#!/bin/sh
printf '%s\\n' aarch64-apple-darwin x86_64-apple-darwin
`);
  executable(join(bin, "xcrun"), `#!/bin/sh
if [ "$1" = "--sdk" ] && [ "$3" = "--show-sdk-path" ]; then
  printf '%s\\n' "$MOCK_SDK"
  exit 0
fi
if [ "$1" = "--sdk" ] && [ "$3" = "--find" ]; then
  printf '%s\\n' /usr/bin/clang
  exit 0
fi
if [ "$1" = "lipo" ]; then
  if [ "$2" = "-create" ]; then
    printf 'lipo %s\\n' "$*" >> "$MOCK_LOG"
    source="$3"
    output=""
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "-output" ]; then output="$2"; break; fi
      shift
    done
    cp "$source" "$output"
    exit $?
  fi
  candidate=""
  for argument in "$@"; do candidate="$argument"; done
  if grep -q 'SINGLE' "$candidate" 2>/dev/null; then
    printf '%s\\n' arm64
  else
    printf '%s\\n' 'arm64 x86_64'
  fi
  exit 0
fi
printf '%s\\n' 'unexpected xcrun invocation' >&2
exit 2
`);
  executable(join(bin, "cargo"), `#!/bin/sh
printf 'cargo %s\\n' "$*" >> "$MOCK_LOG"
target=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--target" ]; then target="$2"; break; fi
  shift
done
mkdir -p "target/$target/release"
for product in mixrelay-service mixrelay-driver-service; do
  printf '%s\\n' 'MACHO universal' > "target/$target/release/$product"
done
printf 'MACHO universal desktop %s\\n' "$target" > "target/$target/release/mixrelay-desktop"
for helper in mixrelay-macos-installer mixrelay-macos-payload-check mixrelay-driver-uninstaller; do
  if [ "\${MOCK_HELPER_SINGLE:-}" = 1 ]; then
    printf '%s\\n' 'MACHO SINGLE' > "target/$target/release/$helper"
  else
    printf '%s\\n' 'MACHO universal' > "target/$target/release/$helper"
  fi
  chmod 755 "target/$target/release/$helper"
done
`);
  executable(join(bin, "plutil"), `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2);
const path = args.at(-1);
fs.appendFileSync(process.env.MOCK_LOG, 'plutil ' + args.join(' ') + '\\n');
if (args[0] === '-create') fs.writeFileSync(path, '{}');
else if (args[0] === '-insert') {
  const value = JSON.parse(fs.readFileSync(path, 'utf8'));
  value[args[1]] = args[2] === '-bool' ? args[3] === 'YES' : args[3];
  fs.writeFileSync(path, JSON.stringify(value));
} else if (args[0] === '-extract') {
  process.stdout.write(JSON.parse(fs.readFileSync(path, 'utf8'))[args[1]]);
} else process.exit(2);
`);
  executable(join(bin, "file"), `#!/bin/sh
if [ "\${MOCK_FILE_FAIL:-}" = 1 ]; then exit 127; fi
candidate="$2"
if grep -q 'MACHO' "$candidate" 2>/dev/null; then
  printf '%s\\n' 'Mach-O universal binary'
else
  printf '%s\\n' 'ASCII text'
fi
`);
  executable(join(bin, "pnpm"), `#!/bin/sh
printf 'pnpm %s\\n' "$*" >> "$MOCK_LOG"
printf 'APPLE_SIGNING_IDENTITY=%s\\n' "\${APPLE_SIGNING_IDENTITY:-}" >> "$MOCK_LOG"
printf 'MIXRELAY_CODESIGN_IDENTITY=%s\\n' "\${MIXRELAY_CODESIGN_IDENTITY:-}" >> "$MOCK_LOG"
# Only the Tauri bundle step produces the app; UI, icon and Driver steps only log.
case "$*" in
  *"tauri bundle"*) ;;
  *"desktop:prepare-driver"*) [ "\${MOCK_PREPARE_DRIVER_FAIL:-}" != 1 ]; exit $? ;;
  *) exit 0 ;;
esac
# Tauri bundles the staged Universal desktop binary rather than compiling one.
[ -f target/universal-apple-darwin/release/mixrelay-desktop ] || exit 3
if [ "\${MOCK_PNPM_MODE:-universal}" = missing ]; then exit 0; fi
mkdir -p "$MOCK_APP/Contents/MacOS"
printf '%s\\n' '{"CFBundleShortVersionString":"1.2.3"}' > "$MOCK_APP/Contents/Info.plist"
if [ "\${MOCK_PNPM_MODE:-universal}" = single ]; then
  printf '%s\\n' 'MACHO SINGLE' > "$MOCK_APP/Contents/MacOS/MixRelay"
else
  cp target/universal-apple-darwin/release/mixrelay-desktop "$MOCK_APP/Contents/MacOS/MixRelay"
fi
if [ "\${MOCK_PNPM_MODE:-universal}" = nested ]; then
  mkdir -p "$MOCK_APP/Contents/Frameworks"
  printf '%s\\n' 'MACHO universal' > "$MOCK_APP/Contents/Frameworks/Helper"
fi
if [ "\${APPLE_SIGNING_IDENTITY:-}" = - ]; then
  : > "$MOCK_APP/.ad-hoc-signed"
elif [ -n "\${APPLE_SIGNING_IDENTITY:-}" ]; then
  : > "$MOCK_APP/.tauri-signed"
fi
`);
  executable(join(bin, "codesign"), `#!/bin/sh
printf 'codesign %s\\n' "$*" >> "$MOCK_LOG"
if [ "$1" = "--verify" ]; then
  [ "\${MOCK_INVALID_SIGNATURE:-}" != 1 ] || exit 1
  candidate=""
  for argument in "$@"; do candidate="$argument"; done
  [ -f "$candidate/.tauri-signed" ] || [ -f "$candidate/.ad-hoc-signed" ] || grep -q SIGNED "$candidate" 2>/dev/null
  exit $?
fi
if [ "$1" = "--force" ] && [ "$2" = "--sign" ]; then
  candidate=""
  for argument in "$@"; do candidate="$argument"; done
  if [ -d "$candidate" ]; then
    : > "$candidate/.ad-hoc-signed"
  else
    printf '%s\\n' SIGNED >> "$candidate"
  fi
  exit 0
fi
exit 0
`);

  return { root, bin, developer, app, unrelated, log, sdk, script };
}

function runBuild(paths, extraEnv = {}, args = []) {
  const env = { ...process.env };
  for (const key of [
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_JOBS",
    "MIXRELAY_CODESIGN_IDENTITY",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_INSTALLER_SIGNING_IDENTITY",
    "CC_aarch64_apple_darwin",
    "CC_x86_64_apple_darwin",
    "CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER",
    "CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER",
    "SDKROOT",
    "MACOSX_DEPLOYMENT_TARGET",
  ]) delete env[key];
  Object.assign(env, {
    PATH: `${paths.bin}:${env.PATH ?? ""}`,
    DEVELOPER_DIR: paths.developer,
    CARGO_BUILD_JOBS: "1",
    MOCK_APP: paths.app,
    MOCK_FILE_FAIL: "",
    MOCK_LOG: paths.log,
    MOCK_PNPM_MODE: "universal",
    MOCK_SDK: paths.sdk,
    APPLE_SIGNING_IDENTITY: "Developer ID Application: MixRelay (AB12C3D4E5)",
    APPLE_INSTALLER_SIGNING_IDENTITY: "Developer ID Installer: MixRelay (AB12C3D4E5)",
    ...extraEnv,
  });
  return spawnSync("zsh", [paths.script, ...args], { cwd: paths.root, encoding: "utf8", env });
}

function log(paths) {
  return existsSync(paths.log) ? readFileSync(paths.log, "utf8") : "";
}

function assertPreserved(paths) {
  assert.equal(readFileSync(join(paths.app, "Contents", "MacOS", "MixRelay"), "utf8"), "MACHO old\n");
  assert.equal(readFileSync(paths.unrelated, "utf8"), "preserve me\n");
}

zshTest("maps explicit identity before Tauri and does not force-resign valid output", (t) => {
  const paths = fixture(t);
  const result = runBuild(paths, { MIXRELAY_CODESIGN_IDENTITY: "Developer ID Application: MixRelay (AB12C3D4E5)" });
  assert.equal(result.status, 0, result.stderr);
  assert.match(log(paths), /tauri bundle --bundles app --target universal-apple-darwin --features custom-protocol/);
  assert.ok(log(paths).indexOf("launcher built") < log(paths).indexOf("tauri bundle"));
  assert.ok(log(paths).indexOf("engine built --prebuilt") < log(paths).indexOf("tauri bundle"));
  assert.match(log(paths), /APPLE_SIGNING_IDENTITY=Developer ID Application: MixRelay/);
  assert.ok(!log(paths).split("\n").some(line => line.startsWith("codesign --force") && line.endsWith("/MixRelay.app")));
  // The removal app's version comes from the mocked plutil, never the host tool.
  assert.match(log(paths), /^plutil -extract CFBundleShortVersionString raw -o - .*\/MixRelay\.app\/Contents\/Info\.plist$/m);
  assert.equal(readFileSync(paths.unrelated, "utf8"), "preserve me\n");
});

zshTest("rejects conflicting signing identities before invoking Tauri", (t) => {
  const paths = fixture(t);
  const result = runBuild(paths, {
    MIXRELAY_CODESIGN_IDENTITY: "one",
    APPLE_SIGNING_IDENTITY: "two",
  });
  assert.notEqual(result.status, 0);
  assert.doesNotMatch(log(paths), /^pnpm /m);
});

zshTest("complete product builds helpers and engine with one public app and remover", (t) => {
  const paths = fixture(t);
  const identity = "Developer ID Application: MixRelay (AB12C3D4E5)";
  const result = runBuild(paths, { APPLE_SIGNING_IDENTITY: identity }, []);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(log(paths), /tauri\.driver\.conf\.json/);
  assert.ok(existsSync(paths.app));
  const commands = log(paths);
  // One cargo build per architecture compiles every release binary; nothing later runs cargo.
  const cargoBuilds = commands.split("\n").filter(line => line.startsWith("cargo "));
  assert.equal(cargoBuilds.length, 2);
  for (const [index, target] of ["aarch64-apple-darwin", "x86_64-apple-darwin"].entries())
    assert.equal(
      cargoBuilds[index],
      `cargo build --locked --release --target ${target} -p mixrelay-macos-installer -p mixrelay-service -p mixrelay-driver-service -p mixrelay-desktop --bins --features mixrelay-desktop/custom-protocol`,
    );
  // The UI and icons are compiled into the desktop binary, so they precede cargo.
  assert.ok(commands.indexOf("pnpm desktop:icons") < commands.indexOf(cargoBuilds[0]));
  assert.ok(commands.indexOf("pnpm --dir apps/control-ui build") < commands.indexOf(cargoBuilds[0]));
  assert.ok(commands.indexOf(cargoBuilds[1]) < commands.indexOf("pnpm desktop:prepare-driver --profile release --prebuilt"));
  assert.ok(commands.indexOf("pnpm desktop:prepare-driver") < commands.indexOf("tauri bundle"));
  const desktopLipo = commands.split("\n").find(line => line.startsWith("lipo ") && line.endsWith("/universal-apple-darwin/release/mixrelay-desktop"));
  assert.ok(desktopLipo, "the desktop binary is merged before bundling");
  assert.match(desktopLipo, /\/aarch64-apple-darwin\/release\/mixrelay-desktop .*\/x86_64-apple-darwin\/release\/mixrelay-desktop -output /);
  assert.ok(commands.indexOf(desktopLipo) < commands.indexOf("tauri bundle"));
  // The bundle carries the staged desktop binary (the mock lipo keeps its first slice).
  assert.equal(readFileSync(join(paths.app, "Contents/MacOS/MixRelay"), "utf8"), "MACHO universal desktop aarch64-apple-darwin\n");
  assert.equal(existsSync(join(paths.app, "Contents/Helpers/Uninstall MixRelay.app")), false);
  assert.ok(existsSync(join(dirname(paths.app),"Uninstall MixRelay.app")));
  assert.doesNotMatch(commands, /removal-packages|build-macos-removal-pkgs/);
  assert.match(commands, /--identifier mixrelay-macos-installer /);
  assert.match(commands, /--identifier mixrelay-macos-payload-check /);
  // APPLE_SIGNING_IDENTITY alone must still reach the engine builder, which otherwise signs ad hoc.
  assert.match(commands, new RegExp(`^engine identity ${identity.replace(/[()]/g, "\\$&")}$`, "m"));
});

zshTest("ad-hoc check builds compile and bundle every product without release signing", (t) => {
  const paths = fixture(t);
  const result = runBuild(paths, { APPLE_SIGNING_IDENTITY: "-", APPLE_INSTALLER_SIGNING_IDENTITY: "" }, []);
  assert.equal(result.status, 0, result.stderr);
  const commands = log(paths);
  assert.match(commands, /tauri bundle --bundles app --target universal-apple-darwin/);
  assert.match(commands, /^APPLE_SIGNING_IDENTITY=-$/m);
  assert.match(commands, /^engine identity -$/m);
  const helpers = commands.split("\n").filter(line => line.startsWith("codesign --force") && line.includes("--identifier mixrelay-"));
  assert.equal(helpers.length, 3);
  for (const line of helpers) {
    assert.match(line, /^codesign --force --sign - --identifier /);
    assert.doesNotMatch(line, /--timestamp|Developer ID/);
  }
});

zshTest("rejects an unsigned release identity before invoking Tauri", (t) => {
  const paths = fixture(t);
  const result = runBuild(paths, { APPLE_SIGNING_IDENTITY: "" });
  assert.notEqual(result.status, 0);
  assert.doesNotMatch(log(paths), /^pnpm /m);
});

zshTest("Driver product rejects a missing signing Team ID before building helpers", (t) => {
  const paths = fixture(t);
  const result = runBuild(paths, { APPLE_SIGNING_IDENTITY: "Developer ID Application: MixRelay" }, []);
  assert.notEqual(result.status, 0);
  assert.doesNotMatch(log(paths), /^cargo |^pnpm /m);
});

zshTest("Driver product rejects single-architecture helper output and restores its manager", (t) => {
  const paths = fixture(t);
  createApp(paths.app, { architecture: "old", signed: true });
  const result = runBuild(paths, {
    APPLE_SIGNING_IDENTITY: "Developer ID Application: MixRelay (AB12C3D4E5)",
    MOCK_HELPER_SINGLE: "1",
  }, []);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Driver helper must contain exactly arm64 and x86_64/);
  assertPreserved(paths);
  assert.doesNotMatch(log(paths), /tauri bundle/);
});

zshTest("rejects nested unsigned output and restores the old app", (t) => {
  const paths = fixture(t);
  createApp(paths.app, { architecture: "old", signed: true });
  const result = runBuild(paths, { MOCK_PNPM_MODE: "nested", MOCK_INVALID_SIGNATURE: "1" });
  assert.notEqual(result.status, 0);
  assertPreserved(paths);
  assert.ok(!log(paths).split("\n").some(line => line.startsWith("codesign --force") && line.endsWith("/MixRelay.app")));
});

zshTest("fails on missing fresh output and restores the old app", (t) => {
  const paths = fixture(t);
  createApp(paths.app, { architecture: "old", signed: true });
  const result = runBuild(paths, { MOCK_PNPM_MODE: "missing" });
  assert.notEqual(result.status, 0);
  assertPreserved(paths);
});

zshTest("fails on a single-architecture output and restores the old app", (t) => {
  const paths = fixture(t);
  createApp(paths.app, { architecture: "old", signed: true });
  const result = runBuild(paths, { MOCK_PNPM_MODE: "single" });
  assert.notEqual(result.status, 0);
  assertPreserved(paths);
});

for (const [step, env] of [["engine merge", { MOCK_ENGINE_FAIL: "1" }], ["Driver preparation", { MOCK_PREPARE_DRIVER_FAIL: "1" }]]) {
  zshTest(`stops before bundling and restores the old app when ${step} fails`, (t) => {
    const paths = fixture(t);
    createApp(paths.app, { architecture: "old", signed: true });
    const result = runBuild(paths, env);
    assert.notEqual(result.status, 0);
    assertPreserved(paths);
    assert.doesNotMatch(log(paths), /tauri bundle/);
  });
}

zshTest("restores the old app when a validation tool is missing", (t) => {
  const paths = fixture(t);
  createApp(paths.app, { architecture: "old", signed: true });
  const result = runBuild(paths, { MOCK_FILE_FAIL: "1" });
  assert.notEqual(result.status, 0);
  assertPreserved(paths);
});

zshTest("rejects a pre-existing app symlink without invoking Tauri", (t) => {
  const paths = fixture(t);
  const linkedApp = join(paths.root, "linked-app");
  createApp(linkedApp, { architecture: "old", signed: true });
  symlinkSync(linkedApp, paths.app);
  const result = runBuild(paths);
  assert.notEqual(result.status, 0);
  assert.equal(lstatSync(paths.app).isSymbolicLink(), true);
  assert.doesNotMatch(log(paths), /^pnpm /m);
});


test("the editor bundles the launcher and icon catalog and no login agent", () => {
  const editor = JSON.parse(readFileSync(new URL("../apps/desktop/src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  assert.deepEqual(editor.bundle.macOS.files, {
    "Resources/mixrelay-driver-launcher": "../../../target/universal-apple-darwin/release/mixrelay-driver-launcher",
    "Resources/Assets.car": "../../../target/app-icon/macos/Assets.car",
  });
});
