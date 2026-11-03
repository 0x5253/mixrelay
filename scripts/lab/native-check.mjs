import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { constants, platform, arch, release } from "node:os";
import { isAbsolute, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { desktopCodeCheckEnvironment } from "../core/desktop-build-policy.mjs";
import { loadContract } from '../support-contract.mjs';
import { observeEnvironment } from '../e2e/environment.mjs';
import { validateCodeCheckEnvironment } from '../core/environment-policy.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

const FOCUSED_CARGO_COMMANDS = new Set(["test", "check", "clippy", "build", "doc", "llvm-cov", "miri", "fmt"]);
const DISALLOWED_CARGO_ACTIONS = new Set(["run", "install", "bundle", "bundling"]);
// Script tests exercise packaging tools with mocked commands; the code-check environment
// disables packaging, so they run with the caller's environment instead.
const LAUNCHD_FIXTURES = ["node", "--test", "scripts/ci/macos-removal-launchd.test.mjs"];
const CALLER_ENVIRONMENT_STEPS = new Set(["pnpm test:scripts", LAUNCHD_FIXTURES.join(" ")]);
const FIXTURE_LIBRARIES = [
  { name: "mixrelay_macos_transport", source: "native/macos/transport/lib.rs",
    manifest: "native/macos/transport/Cargo.toml", variable: "MIXRELAY_NATIVE_TRANSPORT_STATICLIB" },
  { name: "mixrelay_macos_hal", source: "native/macos/hal-driver/rust/src/lib.rs",
    manifest: "native/macos/hal-driver/rust/Cargo.toml", variable: "MIXRELAY_NATIVE_HAL_STATICLIB" },
];

function needsNativeFixtures(args) {
  if (args[0] === "fmt") return false;
  const options = args.slice(0, args.indexOf("--") < 0 ? args.length : args.indexOf("--"));
  const packages = [];
  for (let index = 0; index < options.length; index += 1) {
    if (["-p", "--package"].includes(options[index])) packages.push(options[++index]);
    else if (options[index].startsWith("--package=")) packages.push(options[index].slice(10));
  }
  if (options.includes("--workspace") || options.includes("--all") || packages.length === 0) return true;
  if (packages.includes("mixrelay-macos-test-support")) return true;
  return packages.some(name => ["mixrelay-service", "mixrelay-macos-engine"].includes(name))
    && (["test", "llvm-cov", "miri"].includes(options[0])
      || options.some(option => ["--all-targets", "--tests", "--benches", "--examples"].includes(option)));
}

function fixtureBuildOptions(args) {
  const options = args.slice(0, args.indexOf("--") < 0 ? args.length : args.indexOf("--"));
  const forwarded = options.filter(option => ["--locked", "--offline", "--frozen", "--release"].includes(option));
  for (let index = 0; index < options.length; index += 1) {
    if (["--target-dir", "--profile"].includes(options[index])) forwarded.push(options[index], options[++index]);
    else if (options[index].startsWith("--target-dir=") || options[index].startsWith("--profile=")) forwarded.push(options[index]);
  }
  return forwarded;
}

function nativeFixtureEnvironment(command, args, run, env) {
  const options = { cwd: root, env, shell: false, encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"], maxBuffer: 32 * 1024 * 1024 };
  const host = run(env.RUSTC ?? "rustc", ["--print", "host-tuple"], options);
  if (statusForChild(host) !== 0 || !host.stdout?.trim()) throw new Error("Cannot determine native fixture HOST target");
  const result = run(command, ["build", "--lib", "-p", "mixrelay-macos-transport", "-p", "mixrelay-macos-hal",
    "--target", host.stdout.trim(), "--message-format=json", ...fixtureBuildOptions(args)], options);
  if (statusForChild(result) !== 0) throw new Error(`HOST Cargo fixture library preparation failed (${statusForChild(result)})`);
  const artifacts = new Map(FIXTURE_LIBRARIES.map(library => [library.variable, new Set()]));
  for (const line of (result.stdout ?? "").split("\n")) {
    if (!line.startsWith("{")) continue;
    const message = JSON.parse(line);
    if (message.reason === "compiler-message" && message.message?.rendered) console.error(message.message.rendered);
    if (message.reason !== "compiler-artifact" || message.profile?.test) continue;
    for (const library of FIXTURE_LIBRARIES) {
      if (message.manifest_path !== resolve(root, library.manifest)
        || message.target?.src_path !== resolve(root, library.source)
        || message.target?.name !== library.name || !message.target.crate_types?.includes("staticlib")) continue;
      for (const filename of message.filenames ?? []) {
        if (isAbsolute(filename) && filename.endsWith(".a")) artifacts.get(library.variable).add(filename);
      }
    }
  }
  const prepared = { ...env };
  for (const library of FIXTURE_LIBRARIES) {
    const paths = [...artifacts.get(library.variable)];
    if (paths.length !== 1) throw new Error(`Expected one current HOST Cargo staticlib artifact for ${library.name}`);
    const metadata = statSync(paths[0]);
    if (!metadata.isFile() || metadata.size === 0) throw new Error(`Invalid HOST Cargo staticlib artifact for ${library.name}`);
    prepared[library.variable] = paths[0];
  }
  return prepared;
}

export function buildPlan(os, mode) {
  if (!["darwin", "linux"].includes(os)) throw new Error(`Unsupported OS: ${os}`);
  if (!["build", "test"].includes(mode)) throw new Error("Expected build or test");
  const commands = [
    ["node", "scripts/support-contract.mjs", "check"],
    ["rustc", "--version", "--verbose"],
    ["pnpm", "--version"],
    ["pnpm", "--filter", "@mixrelay/control-ui", "check:boundaries"],
    ["pnpm", "check:crate-layers"],
    ["pnpm", "build"],
  ];
  commands.push(["cargo", "build", "--locked", "--workspace"]);
  if (mode === "test") {
    const platformManifest = `vendor/truce-rack-${os === "darwin" ? "au" : "lv2"}/Cargo.toml`;
    // These platform crates are excluded workspace dependencies. Their own locks
    // and manifest commands include unit tests omitted by workspace selection.
    const platformOptions = ["--locked", "--manifest-path", platformManifest,
      "--target-dir", "target", "--config",
      `patch.crates-io.truce-rack-core.path=${JSON.stringify(resolve(root, "vendor/truce-rack-core"))}`];
    // Style and lints gate before tests, so CI enforces what the Rust rules document claims.
    commands.push(["cargo", "fmt", "--all", "--check"]);
    commands.push(["cargo", "fmt", "--manifest-path", platformManifest, "--check"]);
    commands.push(["cargo", "clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings"]);
    commands.push(["cargo", "clippy", ...platformOptions, "--all-targets", "--", "-D", "warnings"]);
    commands.push(["cargo", "test", "--locked", "-p", "truce-rack-vst3", "-p", "truce-rack-clap", "--lib"]);
    commands.push(os === "darwin"
      ? ["cargo", "test", ...platformOptions, "--lib"]
      : ["bash", "scripts/check-lv2-boundary.sh"]);
    // Platform adapters live in the engine binary, not its portable library.
    // Omitting --bins silently skips platform adapter tests.
    commands.push(["cargo", "test", "--locked", "--workspace", "--lib", "--bins"]);
    if (os === "linux") {
      commands.push(["bash", "native/linux/pipewire/tests/run-stream-buffer-metadata.sh"]);
    }
    commands.push(["pnpm", "--filter", "@mixrelay/control-ui", "exec", "vitest", "run"]);
    if (os === "darwin") {
      commands.push(["pnpm", "check:native"]);
      // The round-trip probe's analysis is checked on synthetic recordings; no device is opened.
      commands.push(["pnpm", "probe:macos-roundtrip", "--self-test"]);
      // Build, signing and release tooling is tested with mocked Apple tools. Some of these
      // tests run the zsh universal-build scripts, so they run on macOS only.
      commands.push(["pnpm", "test:scripts"]);
      commands.push(["node", "--test", "scripts/ci/macos-input-permission.test.mjs",
        "scripts/ci/macos-ui-runner-native.test.mjs"]);
      // Real XPC lifetime fixtures register temporary user launchd jobs; CI only.
      commands.push(LAUNCHD_FIXTURES);
    }
    commands.push(["bash", "scripts/check-pcm-native.sh"]);
  }
  return commands;
}

export function runChecks(os, mode, run, env = process.env) {
  let cargoEnv = desktopCodeCheckEnvironment(env);
  let fixturesPrepared = false;
  const steps = [];
  const qualification = {
    status: "passed", steps,
    scope: "development build/test checks; not installed-product qualification",
    cargoProfile: "dev",
    notVerified: ["driver install/signing", "live audio routing", "plugin editor UI",
      "installed plugin discovery/PCM/parameter/state recall/removal",
      "hardware latency/dropouts", "memory endurance", "release packaging"],
  };
  for (const [command, ...args] of buildPlan(os, mode)) {
    if (os === "darwin" && command === "cargo" && !fixturesPrepared && needsNativeFixtures(args)) {
      try {
        cargoEnv = nativeFixtureEnvironment(cargoExecutable(cargoEnv), args, run, cargoEnv);
        fixturesPrepared = true;
      } catch (error) {
        steps.push({ command: ["cargo", "build", "HOST fixture staticlibs"], exitCode: 1, error: error.message });
        return { ...qualification, status: "failed" };
      }
    }
    const start = performance.now();
    const options = {
      cwd: root,
      stdio: "inherit",
      // pnpm checks can themselves launch Cargo; keep the same code-only scope.
      env: CALLER_ENVIRONMENT_STEPS.has([command, ...args].join(" ")) ? env : cargoEnv,
      shell: false,
    };
    if (command === "cargo") {
      options.shell = false;
    }
    const result = run(command, args, options);
    steps.push({ command: [command, ...args], exitCode: result.status,
      error: result.error?.message, signal: result.signal,
      durationMs: Math.round(performance.now() - start) });
    if (result.error || result.status !== 0) return { ...qualification, status: "failed" };
  }
  return qualification;
}

function focusedCargoArguments(args) {
  if (args[0] !== "cargo" || !args[1]) {
    throw new Error("Expected: cargo <test|check|clippy|build|doc|llvm-cov|miri test|fmt ...>");
  }
  const cargoArgs = args.slice(1);
  const command = cargoArgs[0];
  if (DISALLOWED_CARGO_ACTIONS.has(command) || !FOCUSED_CARGO_COMMANDS.has(command)) {
    throw new Error(`Unsupported focused Cargo command: ${command}`);
  }
  if (command === "fmt" && (!cargoArgs.includes("--check") || cargoArgs.includes("--"))) {
    throw new Error("Focused cargo fmt requires --check without forwarded rustfmt arguments");
  }
  if (command === "llvm-cov" && DISALLOWED_CARGO_ACTIONS.has(cargoArgs[1])) {
    throw new Error(`Unsupported focused cargo llvm-cov action: ${cargoArgs[1]}`);
  }
  if (command === "miri" && cargoArgs[1] !== "test") {
    throw new Error("Focused cargo miri requires the test action");
  }
  return cargoArgs;
}

function cargoExecutable(env) {
  if (env.CARGO === undefined) return "cargo";
  if (typeof env.CARGO !== "string" || env.CARGO.trim().length === 0 || !isAbsolute(env.CARGO)) {
    throw new Error("CARGO must be an absolute executable path when set");
  }
  return env.CARGO;
}

function statusForChild(result) {
  if (result?.error || !result) return 1;
  if (result.signal) {
    const signalNumber = constants.signals?.[result.signal];
    return Number.isInteger(signalNumber) ? 128 + signalNumber : 1;
  }
  return Number.isInteger(result.status) ? result.status : 1;
}

export function runFocusedCargoCheck(
  args,
  { env = process.env, run = spawnSync, log = console.log, reportError = console.error, os = platform() } = {},
) {
  try {
    const cargoArgs = focusedCargoArguments(args);
    const command = cargoExecutable(env);
    let cargoEnv = desktopCodeCheckEnvironment(env);
    log("Focused Cargo code check only; no desktop resources, installer, or product qualification.");
    log(`> ${command} ${cargoArgs.join(" ")}`);
    if (os === "darwin" && needsNativeFixtures(cargoArgs)) {
      cargoEnv = nativeFixtureEnvironment(command, cargoArgs, run, cargoEnv);
    }
    const result = run(command, cargoArgs, {
      cwd: root,
      env: cargoEnv,
      stdio: "inherit",
      shell: false,
    });
    if (result?.error) reportError(`Cargo code check failed to start: ${result.error.message}`);
    else if (result?.signal) reportError(`Cargo code check terminated by ${result.signal}`);
    else if (!Number.isInteger(result?.status)) reportError("Cargo code check returned no exit status");
    return statusForChild(result);
  } catch (error) {
    reportError(error instanceof Error ? error.message : String(error));
    return 1;
  }
}

function main() {
  const args = process.argv.slice(2);
  if (args[0] === "cargo") {
    process.exitCode = runFocusedCargoCheck(args);
    return;
  }
  const mode = process.argv[2] ?? "build";
  // Validate before creating reports or starting subprocesses.
  buildPlan(platform(), mode);
  desktopCodeCheckEnvironment(process.env);
  let distro;
  if (platform() === "linux") distro = readFileSync("/etc/os-release", "utf8");
  const startedAt = new Date().toISOString();
  const environment = observeEnvironment();
  if (process.env.MIXRELAY_CI_TARGET) validateCodeCheckEnvironment(environment, process.env.MIXRELAY_CI_TARGET, loadContract().contract);
  const result = runChecks(platform(), mode, (command, args, options) => {
    console.log(`\n> ${command} ${args.join(" ")}`);
    return spawnSync(command, args, options);
  }, process.env);
  const sourceRevision = process.env.MIXRELAY_SOURCE_REVISION
    ?? spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).stdout?.trim();
  const report = { startedAt, endedAt: new Date().toISOString(), mode,
    sourceRevision, contractSha256: loadContract().sha256, compilerCache: process.env.MIXRELAY_COMPILER_CACHE,
    environment,
    os: platform(), arch: arch(), kernel: release(), distro, ...result };
  const directory = resolve(root, ".lab/reports");
  mkdirSync(directory, { recursive: true });
  const path = resolve(directory, `${startedAt.replaceAll(":", "-")}-${platform()}-${arch()}.json`);
  writeFileSync(path, `${JSON.stringify(report, null, 2)}\n`, { flag: "wx" });
  console.log(`\n${result.status}: ${path}\nDevelopment checks only; installed app/plugin, real-time audio, and GUI are NOT qualified.`);
  process.exitCode = result.status === "passed" ? 0 : 1;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
