import assert from "node:assert/strict";
import * as nodeFs from "node:fs";
import { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const DOCUMENTS = ["LICENSE.txt"];

import {
  EXPECTED_MANAGER_IDENTIFIER,
  EXPECTED_HAL_IDENTIFIER,
  DRIVER_DESTINATION,
  parseArgs,
  preparePayload,
} from "./prepare-macos-release-payload.mjs";

function fsAdapter(simulatedUmask = 0) {
  const calls = [];
  const methods = ["closeSync", "copyFileSync", "lstatSync", "mkdirSync", "openSync", "readSync", "readdirSync", "writeFileSync"];
  return {
    calls,
    ...Object.fromEntries(methods.map(name => [name, (...args) => {
      calls.push([name, ...args]);
      if (name === "mkdirSync" && simulatedUmask !== 0) {
        const [path, options = {}] = args;
        return nodeFs.mkdirSync(path, {
          ...options,
          mode: (options.mode ?? 0o777) & ~simulatedUmask,
        });
      }
      return nodeFs[name](...args);
    }])),
    chmodSync(...args) {
      calls.push(["chmodSync", ...args]);
      return nodeFs.chmodSync(...args);
    },
  };
}

function fixture(t, simulatedUmask = 0) {
  const fixtureRoot = fileURLToPath(new URL("../target/test-artifacts/", import.meta.url));
  mkdirSync(fixtureRoot, { recursive: true });
  const root = mkdtempSync(join(fixtureRoot, "mixrelay-macos-payload-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const manager = join(root, "MixRelay.app");
  const hal = join(root, "MixRelayHAL.driver");
  const driver = join(root, "mixrelay-driver-service");
  const engine = join(root, "mixrelay-service");
  const uninstaller = join(root, "Uninstall MixRelay.app");
  const output = join(root, "payload");
  for (const bundle of [manager, hal, uninstaller]) {
    mkdirSync(join(bundle, "Contents/MacOS"), { recursive: true });
    writeFileSync(join(bundle, "Contents/Info.plist"), "fixture");
  }
  mkdirSync(join(manager, "Contents/Resources/empty"), { recursive: true });
  writeFileSync(join(manager, "Contents/Resources/LICENSE.txt"), "Copyright notice fixture\n원문 그대로\n");
  writeFileSync(join(manager, "Contents/MacOS/mixrelay-desktop"), "manager-binary");
  writeFileSync(join(hal, "Contents/MacOS/MixRelayHAL"), "hal-binary");
  writeFileSync(driver, "driver-binary");
  writeFileSync(engine, "engine-binary");
  chmodSync(engine, 0o755);
  writeFileSync(join(uninstaller, "Contents/MacOS/mixrelay-driver-uninstaller"), "uninstaller-binary");
  chmodSync(join(uninstaller, "Contents/MacOS/mixrelay-driver-uninstaller"), 0o755);
  mkdirSync(join(uninstaller, "Contents/Helpers"));
  writeFileSync(join(uninstaller, "Contents/Helpers/mixrelay-macos-installer"), "helper-binary");
  chmodSync(join(uninstaller, "Contents/Helpers/mixrelay-macos-installer"), 0o755);
  chmodSync(join(manager, "Contents/MacOS/mixrelay-desktop"), 0o755);
  chmodSync(join(hal, "Contents/MacOS/MixRelayHAL"), 0o755);
  chmodSync(driver, 0o755);
  return { root, manager, hal, driver, engine, uninstaller, output, fsApi: fsAdapter(simulatedUmask) };
}

function runner(ids = new Map()) {
  return (command, args) => {
    if (command === "/usr/bin/plutil") {
      const path = args.at(-1);
      const key = args[1];
      return { status: 0, stdout: key === "CFBundleIdentifier" ? ids.get(path) : key === "MixRelaySigningTeam" ? "AB12C3D4E5" : "" };
    }
    if (command === "/usr/bin/file") return { status: 0, stdout: "Mach-O 64-bit dynamically linked shared library" };
    if (command === "/usr/bin/xcrun") return { status: 0, stdout: "arm64 x86_64" };
    throw new Error(`unexpected command ${command}`);
  };
}

function validFixture(t) {
  const paths = fixture(t);
  const ids = new Map([
    [join(paths.manager, "Contents/Info.plist"), EXPECTED_MANAGER_IDENTIFIER],
    [join(paths.hal, "Contents/Info.plist"), EXPECTED_HAL_IDENTIFIER],
    [join(paths.uninstaller, "Contents/Info.plist"), "io.mixrelay.driver.uninstaller"],
  ]);
  return { ...paths, ids };
}

test("parseArgs requires explicit absolute release artifacts", () => {
  assert.deepEqual(parseArgs(["--manager=/release/MixRelay.app", "--hal", "/release/HAL.driver", "--driver", "/release/driver", "--engine", "/release/engine", "--uninstaller", "/release/Uninstall.app", "--output", "/fixture/payload"]), {
    help: false,
    manager: "/release/MixRelay.app",
    hal: "/release/HAL.driver",
    driver: "/release/driver",
    engine: "/release/engine",
    uninstaller: "/release/Uninstall.app",
    output: "/fixture/payload",
  });
  assert.throws(() => parseArgs(["--manager", "relative.app", "--hal", "/b", "--driver", "/c", "--output", "/d"]), /absolute/);
  assert.throws(() => parseArgs(["--manager", "/a", "--hal", "/b", "--driver", "/c", "--engine", "/e"]), /Missing required option: --uninstaller/);
});

test("prepares a deterministic inspectable payload and never creates a pkg", (t) => {
  const paths = validFixture(t);
  const result = preparePayload({ ...paths, run: runner(paths.ids) });
  assert.equal(result.status, "payload-prepared-not-package");
  assert.equal(result.installContract.driver, DRIVER_DESTINATION);
  assert.deepEqual(result.installContract.destinations.find(item => item.path === DRIVER_DESTINATION), {
    path: DRIVER_DESTINATION,
    owner: "root:wheel",
    mode: "0755",
  });
  assert.match(readFileSync(join(paths.output, "templates/io.mixrelay.audio.driver.plist.in"), "utf8"), /__MIXRELAY_SERVICE_USER__/);
  assert.equal(existsSync(join(paths.output, "Applications/MixRelay/MixRelay.app/Contents/Resources/empty")), true);
  for (const filename of DOCUMENTS) {
    assert.deepEqual(
      readFileSync(join(paths.output, "Applications/MixRelay/MixRelay.app/Contents/Resources", filename)),
      readFileSync(join(paths.manager, "Contents/Resources", filename)),
    );
  }
  const manifest = JSON.parse(readFileSync(join(paths.output, "payload-manifest.json"), "utf8"));
  assert.equal(manifest.artifacts.length, 8);
  for (const filename of DOCUMENTS) {
    assert.ok(manifest.artifacts[0].files.some(file => file.path === `Contents/Resources/${filename}`));
  }
  assert.equal(manifest.installContract.requiresPostinstallUserExpansion, true);
  const leases = manifest.installContract.destinations.filter(item => item.path.endsWith(".owner"));
  assert.deepEqual(leases.map(item => [item.path.split("/").at(-1), item.owner, item.mode]), [
    ["app.owner", "<service-user>:<primary-group>", "0644"],
    ["engine.owner", "<service-user>:<primary-group>", "0644"],
    ["installer.owner", "root:wheel", "0644"],
  ]);
  assert.equal(result.installContract.productDirectory, "/Applications/MixRelay");
  assert.ok(readFileSync(join(paths.output, "templates/io.mixrelay.driver.removal.plist"), "utf8").includes("<string>/Applications/MixRelay/Uninstall MixRelay.app/Contents/Helpers/mixrelay-macos-installer</string>"));
  assert.ok(result.installContract.destinations.some(item => item.path === "/Applications/MixRelay" && item.owner === "root:wheel" && item.mode === "0755"));
  assert.equal(result.artifacts.find(item => item.kind === "uninstaller").target, "/Applications/MixRelay/Uninstall MixRelay.app");
  assert.equal(existsSync(join(paths.output, "MixRelay.pkg")), false);
  assert.ok(paths.fsApi.calls.length > 0);
});

test("normalizes staged bundle and wrapper directories under a restrictive umask", (t) => {
  const paths = validFixture(t, 0o077);
  preparePayload({ ...paths, run: runner(paths.ids) });
  const directories = [
    join(paths.output, "Applications"),
    join(paths.output, "Applications/MixRelay"),
    join(paths.output, "Applications/MixRelay/Uninstall MixRelay.app"),
    join(paths.output, "Applications/MixRelay/MixRelay.app"),
    join(paths.output, "Applications/MixRelay/MixRelay.app/Contents"),
    join(paths.output, "Applications/MixRelay/MixRelay.app/Contents/Resources/empty"),
    join(paths.output, "Library"),
    join(paths.output, "Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver"),
    join(paths.output, "Library/Application Support/MixRelay/bin"),
    join(paths.output, "templates"),
  ];
  for (const directory of directories) {
    assert.equal(nodeFs.statSync(directory).mode & 0o777, 0o755, directory);
  }
});

test("rejects known debug layouts before output mutation but allows unrelated debug directories", (t) => {
  const paths = validFixture(t);
  mkdirSync(paths.output);
  for (const driver of [
    join(paths.root, "target", "debug", "mixrelay-driver-service"),
    join(paths.root, "target", "aarch64-apple-darwin", "debug", "mixrelay-driver-service"),
    join(paths.root, "universal", "debug", "mixrelay-driver-service"),
  ]) {
    assert.throws(() => preparePayload({ ...paths, driver, run: runner(paths.ids) }), /debug\/development artifacts/);
  }
  assert.deepEqual(readdirSync(paths.output), []);
  assert.equal(paths.fsApi.calls.length, 0);

  const unrelatedEngine = join(paths.root, "unrelated", "debug", "mixrelay-driver-service");
  mkdirSync(join(paths.root, "unrelated", "debug"), { recursive: true });
  writeFileSync(unrelatedEngine, "driver-binary");
  chmodSync(unrelatedEngine, 0o755);
  assert.equal(preparePayload({ ...paths, driver: unrelatedEngine, run: runner(paths.ids) }).status, "payload-prepared-not-package");
});

test("rejects unexpected identifiers and non-universal architecture", (t) => {
  const paths = validFixture(t);
  const wrongIds = new Map(paths.ids).set(join(paths.hal, "Contents/Info.plist"), "io.other.driver");
  assert.throws(() => preparePayload({ ...paths, run: runner(wrongIds) }), /unexpected CFBundleIdentifier/);
  assert.throws(() => preparePayload({ ...paths, run: (command, args) => {
    if (command === "/usr/bin/plutil") return runner(paths.ids)(command, args);
    if (command === "/usr/bin/file") return { status: 0, stdout: "Mach-O" };
    if (command === "/usr/bin/xcrun") return { status: 0, stdout: "arm64" };
    throw new Error("unexpected command");
  } }), /Expected exactly arm64 and x86_64/);
});

test("rejects symlinked artifacts and non-empty output without touching them", (t) => {
  const paths = validFixture(t);
  symlinkSync(paths.driver, join(paths.root, "driver-link"));
  assert.throws(() => preparePayload({ ...paths, driver: join(paths.root, "driver-link"), run: runner(paths.ids) }), /path must not contain symlinks/);
  mkdirSync(paths.output);
  writeFileSync(join(paths.output, "keep"), "untouched");
  assert.throws(() => preparePayload({ ...paths, run: runner(paths.ids) }), /--output must be empty/);
  assert.equal(readFileSync(join(paths.output, "keep"), "utf8"), "untouched");
});

test("rejects symlinked input and output ancestors", (t) => {
  const inputPaths = validFixture(t);
  const inputAlias = join(inputPaths.root, "artifact-parent");
  symlinkSync(inputPaths.root, inputAlias);
  assert.throws(() => preparePayload({
    ...inputPaths,
    manager: join(inputAlias, "MixRelay.app"),
    run: runner(inputPaths.ids),
  }), /--manager path must not contain symlinks/);

  const outputPaths = validFixture(t);
  const outputAlias = join(outputPaths.root, "output-parent");
  symlinkSync(outputPaths.root, outputAlias);
  assert.throws(() => preparePayload({
    ...outputPaths,
    output: join(outputAlias, "payload"),
    run: runner(outputPaths.ids),
  }), /--output path must not contain symlinks/);
});

test("rejects zero-size executables", (t) => {
  const paths = validFixture(t);
  writeFileSync(paths.driver, "");
  chmodSync(paths.driver, 0o755);
  assert.throws(() => preparePayload({ ...paths, run: runner(paths.ids) }), /driver must be a non-empty executable/);
});

for (const filename of DOCUMENTS) {
  for (const kind of ["missing", "empty"]) {
    test(`rejects ${kind} ${filename} before staging without modifying the manager`, (t) => {
      const paths = validFixture(t);
      const document = join(paths.manager, "Contents/Resources", filename);
      if (kind === "missing") rmSync(document);
      else writeFileSync(document, "");
      const executable = readFileSync(join(paths.manager, "Contents/MacOS/mixrelay-desktop"));
      assert.throws(() => preparePayload({ ...paths, run: runner(paths.ids) }), /Missing artifact path|must be non-empty/);
      assert.equal(existsSync(paths.output), false);
      assert.deepEqual(readFileSync(join(paths.manager, "Contents/MacOS/mixrelay-desktop")), executable);
      assert.equal(paths.fsApi.calls.some(([method]) => ["mkdirSync", "copyFileSync", "writeFileSync"].includes(method)), false);
    });
  }
}

test("rejects staging inside a supplied artifact", (t) => {
  const paths = validFixture(t);
  assert.throws(() => preparePayload({ ...paths, output: join(paths.manager, "payload"), run: runner(paths.ids) }), /separate from manager/);
});

test("rejects overlapping source artifacts before mutating output", (t) => {
  const paths = validFixture(t);
  mkdirSync(paths.output);
  assert.throws(() => preparePayload({
    ...paths,
    hal: paths.manager,
    run: runner(paths.ids),
  }), /manager must be separate from HAL/);
  assert.deepEqual(readdirSync(paths.output), []);
  assert.equal(paths.fsApi.calls.some(([method]) => ["mkdirSync", "copyFileSync", "writeFileSync"].includes(method)), false);
});

test("rejects missing artifacts and failed inspection commands", (t) => {
  const paths = validFixture(t);
  assert.throws(() => preparePayload({ ...paths, manager: join(paths.root, "missing.app"), run: runner(paths.ids) }), /Missing artifact path/);
  const failingRunner = (command, args) => {
    if (command === "/usr/bin/plutil") return runner(paths.ids)(command, args);
    if (command === "/usr/bin/file") return { status: 0, stdout: "Mach-O" };
    return { status: 1, stderr: "lipo rejected fixture" };
  };
  assert.throws(() => preparePayload({ ...paths, run: failingRunner }), /inspect architectures .* failed/);
});
