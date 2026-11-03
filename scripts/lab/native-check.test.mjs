import assert from "node:assert/strict";
import { constants } from "node:os";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { DESKTOP_CODE_CHECK_TAURI_CONFIG } from "../core/desktop-build-policy.mjs";
import { buildPlan, runChecks, runFocusedCargoCheck } from "./native-check.mjs";

const ROOT_DIR = fileURLToPath(new URL("../..", import.meta.url)).replace(/[\\/]+$/, "");

function nativeFixture(t) {
  const directory = join(ROOT_DIR, "target/test-artifacts");
  mkdirSync(directory, { recursive: true });
  const owned = mkdtempSync(join(directory, "native-check-"));
  t.after(() => rmSync(owned, { recursive: true, force: true }));
  const libraries = [
    ["mixrelay_macos_transport", "native/macos/common", "lib.rs"],
    ["mixrelay_hal_core", "native/macos/hal-driver/rust", "src/lib.rs"],
  ].map(([name, source, entry]) => {
    const archive = join(owned, `lib${name}.a`);
    writeFileSync(archive, "fixture archive receipt");
    return { reason: "compiler-artifact", manifest_path: join(ROOT_DIR, source, "Cargo.toml"),
      target: { name, src_path: join(ROOT_DIR, source, entry), crate_types: ["rlib", "staticlib"] },
      profile: { test: false }, filenames: [archive] };
  });
  return { libraries, stdout: () => libraries.map(message => JSON.stringify(message)).join("\n") };
}

function nativeRunner(fixture, invoke) {
  return (command, args, options) => {
    if (args.join(" ") === "--print host-tuple") return { status: 0, stdout: "aarch64-apple-darwin\n" };
    if (args.includes("--message-format=json")) return { status: 0, stdout: fixture.stdout() };
    return invoke(command, args, options);
  };
}

test("public workspace and desktop test commands keep the canonical code-check entry", () => {
  const entry = "node scripts/lab/native-check.mjs cargo ";
  const { scripts } = JSON.parse(readFileSync(new URL("../../package.json", import.meta.url), "utf8"));
  assert.ok(scripts["check:rust"].includes(`${entry}clippy --workspace`));
  assert.ok(scripts["coverage:rust"].startsWith(`${entry}llvm-cov --workspace`));
  // macos-test-support's build script needs the HOST staticlib receipts only this entry prepares.
  assert.ok(scripts["check:macos-engine"].startsWith(`${entry}test -p mixrelay-macos-engine `));
  assert.ok(scripts["check:native"].startsWith(`${entry}test -p mixrelay-macos-transport `));
  for (const name of ["check:native", "check:macos-engine"]) {
    const direct = scripts[name].split("&&").map(part => part.trim())
      .filter(part => /^cargo (?:test|check|clippy|build)\b/.test(part));
    assert.deepEqual(direct, [], `${name} must not run Cargo checks around the fixture entry`);
  }
  for (const script of ["check-macos-application-clock.sh", "check-macos-clock-boundaries.sh"]) {
    const source = readFileSync(new URL(`../${script}`, import.meta.url), "utf8");
    const desktopChecks = source.split("\n")
      .filter(line => /\bcargo test\b.*\s-p mixrelay-desktop(?:\s|$)/.test(line));
    assert.ok(desktopChecks.length > 0);
    assert.ok(desktopChecks.every(line => line.includes(entry)));
  }
});

for (const os of ["linux", "darwin"]) {
  test(`${os} builds the desktop and engine, with locked dependencies`, () => {
    const plan = buildPlan(os, "build");
    const build = plan.findIndex(c => c.join(" ") === "cargo build --locked --workspace");
    const prepare = plan.findIndex(c => c.includes("scripts/prepare-desktop-engine.mjs"));
    assert.ok(build >= 0);
    assert.equal(prepare, -1);
    assert.equal(buildPlan(os, "build").some(c => c.includes("test")), false);
  });
  test(`${os} runs tests without installing a driver`, () => {
    const plan = buildPlan(os, "test");
    assert.ok(plan.some(c => c.includes("test")));
    const clippy = plan.findIndex(c => c[1] === "clippy");
    assert.ok(plan.some(c => c[1] === "fmt" && c.includes("--check")));
    assert.ok(clippy >= 0 && plan[clippy].includes("warnings"));
    assert.ok(clippy < plan.findIndex(c => c[1] === "test"), "lints gate before tests");
    assert.ok(plan.some(c => c.join(" ") === "cargo test --locked --workspace --lib --bins"));
    assert.equal(plan.some(c => c.join(" ").includes("install-")), false);
    assert.equal(plan.some(c => c.join(" ") === "pnpm check:native"), os === "darwin");
    assert.equal(plan.some(c => c.join(" ") === "pnpm probe:macos-roundtrip --self-test"), os === "darwin");
    assert.equal(plan.some(c => c.join(" ") === "pnpm test:scripts"), os === "darwin",
      "build and release script tests need the macOS zsh build scripts");
    assert.equal(plan.some(c => c.join(" ") === "node --test scripts/ci/macos-removal-launchd.test.mjs"), os === "darwin",
      "launchd XPC fixtures run only in the macOS CI plan");
    assert.equal(plan.some(c => c.includes("scripts/ci/macos-ui-runner-native.test.mjs")), os === "darwin",
      "exact-code signing fixtures execute in the macOS CI plan");
    assert.equal(plan.some(c => c.includes("scripts/check-pcm-native.sh")), true);
  });
}
test("invalid modes and hosts fail before executing anything", () => {
  assert.throws(() => buildPlan("linux", "release"));
  assert.throws(() => buildPlan("unsupported", "build"));
});
test("command failure stops the plan and cannot be reported as a pass", () => {
  let calls = 0;
  const result = runChecks("linux", "test", () => ({ status: ++calls === 2 ? 1 : 0 }));
  assert.equal(result.status, "failed");
  assert.equal(calls, 2);
});

test("Linux native callback fixture failures fail the full source gate", () => {
  const fixture = "native/linux/pipewire/tests/run-stream-buffer-metadata.sh";
  const calls = [];
  const result = runChecks("linux", "test", (command, args) => {
    calls.push([command, ...args]);
    return { status: args.includes(fixture) ? 17 : 0 };
  });
  assert.equal(result.status, "failed");
  assert.deepEqual(calls.at(-1), ["bash", fixture]);
  assert.equal(result.steps.at(-1).exitCode, 17);
  assert.equal(buildPlan("darwin", "test").some(step => step.includes(fixture)), false);
});

for (const [os, format] of [["darwin", "au"], ["linux", "lv2"]]) {
  test(`${format} excluded adapter unit failures fail the source gate`, t => {
    const fixture = nativeFixture(t);
    const manifest = `vendor/truce-rack-${format}/Cargo.toml`;
    const calls = [];
    const result = runChecks(os, "test", nativeRunner(fixture, (command, args) => {
      calls.push([command, ...args]);
      return { status: (os === "darwin" ? args[0] === "test" && args.includes(manifest)
        : args.includes("scripts/check-lv2-boundary.sh")) ? 19 : 0 };
    }));
    assert.equal(result.status, "failed");
    assert.equal(result.steps.at(-1).exitCode, 19);
    const command = calls.at(-1);
    if (os === "darwin") {
      assert.equal(command[0], "cargo");
      assert.equal(command[1], "test");
      assert.ok(command.includes("--locked"));
      assert.ok(command.includes("--lib"));
      assert.ok(command.includes(manifest));
      assert.ok(command.includes("--target-dir"));
      assert.ok(command.includes("target"));
    } else {
      assert.deepEqual(command, ["bash", "scripts/check-lv2-boundary.sh"]);
    }
    const plan = buildPlan(os, "test");
    assert.ok(plan.some(step => step[1] === "clippy" && step.includes(manifest)
      && step.includes("--all-targets") && step.includes("warnings")));
    assert.ok(plan.some(step => step[1] === "fmt" && step.includes(manifest)
      && step.includes("--check")));
  });
}
test("missing executables and terminated children are failures", () => {
  for (const result of [{ status: null, error: new Error("ENOENT") }, { status: null, signal: "SIGTERM" }]) {
    assert.equal(runChecks("linux", "build", () => result).status, "failed");
  }
});

test("all broad-check Cargo subprocesses receive the canonical code-only environment", () => {
  const calls = [];
  const env = { TAURI_CONFIG: DESKTOP_CODE_CHECK_TAURI_CONFIG, MIXRELAY_TEST_ENV: "kept" };
  const result = runChecks("linux", "test", (command, args, options) => {
    calls.push({ command, args, options });
    return { status: 0 };
  }, env);
  assert.equal(result.status, "passed");
  const cargoCalls = calls.filter(call => call.command === "cargo");
  assert.ok(cargoCalls.length > 1);
  for (const call of cargoCalls) {
    assert.equal(call.options.cwd, ROOT_DIR);
    assert.equal(call.options.shell, false);
    assert.equal(call.options.env.TAURI_CONFIG, DESKTOP_CODE_CHECK_TAURI_CONFIG);
    assert.equal(call.options.env.MIXRELAY_DESKTOP_CODE_CHECK, "1");
    assert.equal(call.options.env.MIXRELAY_TEST_ENV, "kept");
  }
});

test("nested pnpm native checks inherit the same code-only environment and HOST receipts", t => {
  const fixture = nativeFixture(t);
  let checked = false;
  const result = runChecks("darwin", "test", nativeRunner(fixture, (command, args, options) => {
    if (command === "pnpm" && args[0] === "check:native") {
      checked = true;
      assert.equal(options.env.TAURI_CONFIG, DESKTOP_CODE_CHECK_TAURI_CONFIG);
      assert.equal(options.env.MIXRELAY_DESKTOP_CODE_CHECK, "1");
      assert.equal(options.env.MIXRELAY_NATIVE_TRANSPORT_STATICLIB, fixture.libraries[0].filenames[0]);
      assert.equal(options.env.MIXRELAY_NATIVE_HAL_STATICLIB, fixture.libraries[1].filenames[0]);
    }
    return { status: 0 };
  }), {});
  assert.equal(result.status, "passed");
  assert.equal(checked, true);
});

test("script tests run with the caller environment, not the packaging-disabled code-check one", t => {
  const fixture = nativeFixture(t);
  let checked = false;
  const result = runChecks("darwin", "test", nativeRunner(fixture, (command, args, options) => {
    if (command === "pnpm" && args.join(" ") === "test:scripts") {
      checked = true;
      assert.deepEqual(options.env, { MIXRELAY_TEST_ENV: "kept" });
      assert.equal(options.shell, false);
    }
    return { status: 0 };
  }), { MIXRELAY_TEST_ENV: "kept" });
  assert.equal(result.status, "passed");
  assert.equal(checked, true);
});

test("conflicting Tauri config fails before broad checks spawn anything", () => {
  let calls = 0;
  assert.throws(() => runChecks("linux", "build", () => {
    calls += 1;
    return { status: 0 };
  }, { TAURI_CONFIG: '{"bundle":{"externalBin":["fixture-engine"]}}' }),
  /conflicts with the canonical desktop code-check configuration/);
  assert.equal(calls, 0);
});

test("focused Cargo mode preserves exact argv, environment, and child exit status without a shell", () => {
  const args = ["cargo", "test", "--locked", "-p", "processor-host", "--lib", "--", "--exact", "fixture_case"];
  const logs = [];
  const status = runFocusedCargoCheck(args, {
    env: { CARGO: process.execPath, FIXTURE_ENV: "preserved" },
    log: message => logs.push(message),
    reportError: message => assert.fail(message),
    run: (command, cargoArgs, options) => {
      assert.equal(command, process.execPath);
      assert.deepEqual(cargoArgs, args.slice(1));
      assert.equal(options.cwd, ROOT_DIR);
      assert.equal(options.shell, false);
      assert.equal(options.stdio, "inherit");
      assert.equal(options.env.MIXRELAY_DESKTOP_CODE_CHECK, "1");
      assert.equal(options.env.FIXTURE_ENV, "preserved");
      return { status: 17 };
    },
  });
  assert.equal(status, 17);
  assert.match(logs[0], /code check only/);
});

test("focused Cargo mode defaults to cargo and refuses run, install, and bundle actions", () => {
  let calls = 0;
  const options = {
    os: "linux",
    env: {},
    log: () => {},
    reportError: () => {},
    run: () => { calls += 1; return { status: 0 }; },
  };
  assert.equal(runFocusedCargoCheck(["cargo", "check", "-p", "processor-host"], {
    ...options,
    run: (command, args) => {
      assert.equal(command, "cargo");
      assert.deepEqual(args, ["check", "-p", "processor-host"]);
      calls += 1;
      return { status: 0 };
    },
  }), 0);
  for (const args of [
    ["cargo", "run"], ["cargo", "install", "some-crate"], ["cargo", "bundle"],
    ["cargo", "llvm-cov", "run"], ["cargo", "llvm-cov", "install"],
  ]) {
    assert.equal(runFocusedCargoCheck(args, options), 1, args.join(" "));
  }
  assert.equal(runFocusedCargoCheck(["cargo", "test"], { ...options, env: { CARGO: "cargo" } }), 1);
  assert.equal(calls, 1);
});

test("focused Cargo mode propagates child signal and reports spawn or missing-status errors", () => {
  const errors = [];
  const options = { os: "linux", env: {}, log: () => {}, reportError: message => errors.push(message) };
  const signalStatus = runFocusedCargoCheck(["cargo", "build", "-p", "processor-host"], {
    ...options, run: () => ({ status: null, signal: "SIGTERM" }),
  });
  assert.equal(signalStatus, 128 + (constants.signals.SIGTERM ?? 15));
  assert.match(errors.at(-1), /terminated by SIGTERM/);
  assert.equal(runFocusedCargoCheck(["cargo", "check"], {
    ...options, run: () => ({ status: null, error: new Error("ENOENT") }),
  }), 1);
  assert.match(errors.at(-1), /failed to start: ENOENT/);
  assert.equal(runFocusedCargoCheck(["cargo", "doc"], {
    ...options, run: () => ({ status: null }),
  }), 1);
  assert.match(errors.at(-1), /no exit status/);
});
test("pass and failure reports identify development checks, not installed plugin qualification", () => {
  for (const status of [0, 1]) {
    const result = runChecks("linux", "test", () => ({ status }));
    assert.equal(result.status, status === 0 ? "passed" : "failed");
    assert.equal(result.cargoProfile, "dev");
    assert.equal(result.scope, "development build/test checks; not installed-product qualification");
    assert.ok(result.notVerified.includes("installed plugin discovery/PCM/parameter/state recall/removal"));
    assert.ok(result.notVerified.includes("hardware latency/dropouts"));
    assert.ok(result.notVerified.includes("release packaging"));
  }
});

for (const fixturePackage of ["mixrelay-service", "mixrelay-macos-engine"]) {
test(`macOS ${fixturePackage} fixture checks capture exact HOST Cargo artifacts and preserve the requested target/argv`, t => {
  const fixture = nativeFixture(t);
  const calls = [];
  const args = ["cargo", "clippy", "--locked", "-p", fixturePackage, "--all-targets",
    "--target", "x86_64-apple-darwin", "--", "-D", "warnings"];
  const result = runFocusedCargoCheck(args, {
    os: "darwin", env: { CARGO_BUILD_TARGET: "x86_64-apple-darwin", FIXTURE_ENV: "kept" },
    log: () => {}, reportError: message => assert.fail(message),
    run: (command, cargoArgs, options) => {
      calls.push({ command, args: cargoArgs, options });
      if (cargoArgs.includes("host-tuple")) return { status: 0, stdout: "aarch64-apple-darwin\n" };
      if (cargoArgs.includes("--message-format=json")) return { status: 0, stdout: fixture.stdout() };
      assert.deepEqual(cargoArgs, args.slice(1));
      assert.equal(options.env.MIXRELAY_NATIVE_TRANSPORT_STATICLIB, fixture.libraries[0].filenames[0]);
      assert.equal(options.env.MIXRELAY_NATIVE_HAL_STATICLIB, fixture.libraries[1].filenames[0]);
      assert.equal(options.env.FIXTURE_ENV, "kept");
      assert.equal(options.env.TAURI_CONFIG, DESKTOP_CODE_CHECK_TAURI_CONFIG);
      assert.equal(options.stdio, "inherit");
      return { status: 17 };
    },
  });
  assert.equal(result, 17);
  assert.deepEqual(calls[1].args, ["build", "--lib", "-p", "mixrelay-macos-transport", "-p", "mixrelay-hal-core",
    "--target", "aarch64-apple-darwin", "--message-format=json", "--locked"]);
  assert.equal(calls.every(call => call.options.shell === false), true);
});

}

test("macOS non-fixture package checks and Linux workspace checks have no fixture prebuild", () => {
  for (const [os, args] of [
    ["linux", ["cargo", "test", "--workspace"]],
    ["darwin", ["cargo", "fmt", "--all", "--check"]],
    ["darwin", ["cargo", "test", "-p", "processor-host", "--lib"]],
    ["darwin", ["cargo", "build", "-p", "mixrelay-service"]],
  ]) {
    let calls = 0;
    assert.equal(runFocusedCargoCheck(args, {
      os, env: {}, log: () => {}, reportError: message => assert.fail(message),
      run: (_, cargoArgs) => { calls += 1; assert.deepEqual(cargoArgs, args.slice(1)); return { status: 0 }; },
    }), 0);
    assert.equal(calls, 1);
  }
});

test("fixture preparation requires current exact staticlib artifacts, never a stale filename fallback", t => {
  for (const mutation of [
    messages => messages.pop(),
    messages => { messages[0].target.src_path = join(ROOT_DIR, "other/lib.rs"); },
    messages => { messages[0].target.crate_types = ["rlib"]; },
    messages => { messages[0].filenames.push(messages[1].filenames[0]); },
    messages => { messages[0].profile.test = true; },
    messages => { writeFileSync(messages[0].filenames[0], ""); },
  ]) {
    const fixture = nativeFixture(t);
    mutation(fixture.libraries);
    let requested = false;
    const errors = [];
    assert.equal(runFocusedCargoCheck(["cargo", "test", "-p", "mixrelay-macos-test-support"], {
      os: "darwin", env: {}, log: () => {}, reportError: message => errors.push(message),
      run: nativeRunner(fixture, () => { requested = true; return { status: 0 }; }),
    }), 1);
    assert.equal(requested, false);
    assert.match(errors[0], /HOST Cargo staticlib artifact/);
  }
});

test("failed HOST Cargo preparation prevents the requested fixture check", () => {
  let calls = 0;
  const errors = [];
  assert.equal(runFocusedCargoCheck(["cargo", "check", "--workspace", "--all-targets"], {
    os: "darwin", env: {}, log: () => {}, reportError: message => errors.push(message),
    run: (_, args) => {
      calls += 1;
      if (args.includes("host-tuple")) return { status: 0, stdout: "aarch64-apple-darwin\n" };
      return { status: 9 };
    },
  }), 1);
  assert.equal(calls, 2);
  assert.match(errors[0], /preparation failed \(9\)/);
});

test("focused formatting checks stay read-only and skip native fixture builds on both platforms", () => {
  for (const os of ["darwin", "linux"]) {
    const calls = [];
    const options = { os, env: {}, log: () => {}, reportError: () => {},
      run: (command, args) => { calls.push([command, ...args]); return { status: 0 }; } };
    assert.equal(runFocusedCargoCheck(["cargo", "fmt", "--all", "--check"], options), 0);
    assert.deepEqual(calls, [["cargo", "fmt", "--all", "--check"]]);
    for (const args of [["fmt", "--all"], ["fmt", "--check", "--", "--emit=files"]]) {
      assert.equal(runFocusedCargoCheck(["cargo", ...args], options), 1);
    }
    assert.equal(calls.length, 1);
  }
});


test("focused Miri tests preserve the selected toolchain and never admit execution actions", () => {
  for (const os of ["darwin", "linux"]) {
    const calls = [];
    const args = ["miri", "test", "-p", "mixrelay-processor-host", "--lib", "--", "--exact", "memory_case"];
    const options = {
      os, env: { RUSTUP_TOOLCHAIN: "nightly" }, log: () => {}, reportError: () => {},
      run: (command, received, settings) => {
        assert.equal(settings.env.RUSTUP_TOOLCHAIN, "nightly");
        assert.equal(settings.env.MIXRELAY_DESKTOP_CODE_CHECK, "1");
        assert.equal(settings.shell, false);
        calls.push([command, ...received]);
        return { status: 0 };
      },
    };
    assert.equal(runFocusedCargoCheck(["cargo", ...args], options), 0);
    assert.deepEqual(calls, [["cargo", ...args]]);
    for (const action of ["run", "setup", "clean", "install"]) {
      assert.equal(runFocusedCargoCheck(["cargo", "miri", action], options), 1);
    }
    assert.equal(calls.length, 1);
  }
});
