import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildPlan,
  classifyTarget,
  parseArgs,
  prepareDesktopEngine,
  resolveTarget,
} from "./prepare-desktop-engine.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));

function fakeFs(output) {
  const copies = [];
  const directories = [];
  return {
    copies,
    directories,
    existsSync(path) { return path === output; },
    mkdirSync(path, options) { directories.push({ path, options }); },
    copyFileSync(source, destination) { copies.push({ source, destination }); },
  };
}

test("Linux config bundles the internal Driver runtime and matching engine sidecars", () => {
  const linux = JSON.parse(readFileSync(join(root, "apps/desktop/src-tauri/tauri.linux.conf.json"), "utf8"));
  assert.deepEqual(linux.bundle.externalBin, [
    "binaries/mixrelay-driver-service",
    "binaries/mixrelay-service",
  ]);
});

test("arguments accept only the supported profiles and target form", () => {
  assert.deepEqual(parseArgs(["--profile", "dev", "--target=x86_64-unknown-linux-gnu"]), {
    help: false, profile: "dev", target: "x86_64-unknown-linux-gnu",
  });
  assert.throws(() => parseArgs(["--profile", "debug"]), /Unsupported profile/);
  assert.throws(() => parseArgs(["--target", "../linux"]), /requires a Rust target triple|Invalid/);
  assert.throws(() => parseArgs(["--unknown"]), /Unknown argument/);
});

test("target classification rejects unsupported platforms and cross-platform mismatches", () => {
  assert.equal(classifyTarget("x86_64-unknown-linux-gnu"), "linux");
  assert.equal(classifyTarget("aarch64-apple-darwin"), "darwin");
  assert.throws(() => classifyTarget("x86_64-unknown-freebsd"), /Unsupported desktop target/);
  assert.throws(() => resolveTarget({
    requestedTarget: "aarch64-apple-darwin",
    env: { TAURI_ENV_PLATFORM: "linux" },
  }), /TAURI_ENV_PLATFORM/);
});

test("Tauri's actual target wins over host guessing and explicit targets must agree", () => {
  let rustcCalls = 0;
  const run = () => { rustcCalls += 1; return { status: 0, stdout: "host: aarch64-apple-darwin\n" }; };
  assert.deepEqual(resolveTarget({
    env: { TAURI_ENV_TARGET_TRIPLE: "x86_64-unknown-linux-gnu" }, run,
  }), { target: "x86_64-unknown-linux-gnu", platform: "linux" });
  assert.equal(rustcCalls, 0);
  assert.deepEqual(resolveTarget({
    env: {}, run,
  }), { target: "aarch64-apple-darwin", platform: "darwin" });
  assert.equal(rustcCalls, 1);
  assert.throws(() => resolveTarget({
    requestedTarget: "aarch64-apple-darwin",
    env: { TAURI_ENV_TARGET_TRIPLE: "x86_64-unknown-linux-gnu" },
  }), /does not match/);
});

test("macOS is a no-op and does not invoke Cargo or create an engine payload", () => {
  let calls = 0;
  const result = prepareDesktopEngine({
    args: ["--profile", "release"],
    env: { TAURI_ENV_TARGET_TRIPLE: "aarch64-apple-darwin" },
    run() { calls += 1; return { status: 0 }; },
  });
  assert.equal(result.status, "noop");
  assert.equal(calls, 0);
});

test("dev builds the target-specific Cargo output and copies sidecar plus target/debug engine", () => {
  const target = "x86_64-unknown-linux-gnu";
  const output = join(root, "target", target, "debug", "mixrelay-service");
  const fsApi = fakeFs(output);
  const calls = [];
  const result = prepareDesktopEngine({
    args: ["--profile", "dev", "--target", target],
    env: {},
    fsApi,
    run(command, args, options) { calls.push({ command, args, options }); return { status: 0 }; },
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, "cargo");
  assert.ok(calls[0].args.includes("--locked"));
  assert.deepEqual(calls[0].args.slice(-2), ["--target", target]);
  assert.equal(fsApi.copies.length, 2);
  assert.equal(fsApi.copies[0].destination, join(root, "apps/desktop/src-tauri/binaries", `mixrelay-service-${target}`));
  assert.equal(fsApi.copies[1].destination, join(root, "target/debug/mixrelay-service"));
  assert.equal(result.devAdjacent, join(root, "target/debug/mixrelay-service"));
});

test("Linux release stages only the Tauri sidecar input", () => {
  const target = "aarch64-unknown-linux-gnu";
  const output = join(root, "target", target, "release", "mixrelay-service");
  const fsApi = fakeFs(output);
  const result = prepareDesktopEngine({
    args: ["--profile=release", `--target=${target}`],
    env: {},
    fsApi,
    run() { return { status: 0 }; },
  });
  assert.equal(fsApi.copies.length, 1);
  assert.equal(fsApi.copies[0].source, output);
  assert.equal(fsApi.copies[0].destination, join(root, "apps/desktop/src-tauri/binaries", `mixrelay-service-${target}`));
  assert.equal(result.devAdjacent, undefined);
  assert.ok(result.command.includes("--release"));
});

test("engine build receives the same target directory environment used for locating its output", () => {
  const env = { CARGO_TARGET_DIR: "target/custom", TAURI_ENV_TARGET_TRIPLE: "aarch64-unknown-linux-gnu" };
  const output = join(root, "target/custom/aarch64-unknown-linux-gnu/release/mixrelay-service");
  let observed;
  const result = prepareDesktopEngine({
    env,
    fsApi: fakeFs(output),
    run(_command, _args, options) { observed = options; return { status: 0 }; },
  });
  assert.equal(observed.cwd, root);
  assert.deepEqual(observed.env, env);
  assert.equal(result.output, output);
});

test("failed Cargo builds stop before any copy and preserve the failure status", () => {
  const fsApi = fakeFs("never-created");
  assert.throws(() => prepareDesktopEngine({
    args: ["--profile", "release", "--target", "x86_64-unknown-linux-gnu"],
    env: {},
    fsApi,
    run() { return { status: 23 }; },
  }), error => error?.exitCode === 23 && fsApi.copies.length === 0);
});

test("a successful Cargo result still fails if its expected output is missing", () => {
  const fsApi = fakeFs("different-output");
  assert.throws(() => prepareDesktopEngine({
    args: ["--profile", "dev", "--target", "x86_64-unknown-linux-gnu"],
    env: {},
    fsApi,
    run() { return { status: 0 }; },
  }), /Cargo did not produce/);
});
