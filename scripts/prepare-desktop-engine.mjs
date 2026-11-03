import { spawnSync } from "node:child_process";
import * as defaultFs from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const ENGINE_BINARY_NAME = "mixrelay-service";
export const SIDECAR_CONFIG_PATH = "binaries/mixrelay-service";
export const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const PROFILES = new Set(["dev", "release"]);
const TARGET_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

export function usage() {
  return [
    "Usage: node scripts/prepare-desktop-engine.mjs [--profile dev|release] [--target TARGET_TRIPLE]",
    "",
    "--profile defaults to release. --target defaults to TAURI_ENV_TARGET_TRIPLE,",
    "then rustc's host target when Tauri did not provide a target.",
  ].join("\n");
}

export function parseArgs(args) {
  let profile = "release";
  let target;
  let help = false;

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--help" || arg === "-h") {
      help = true;
      continue;
    }
    if (arg === "--profile" || arg.startsWith("--profile=")) {
      const value = arg === "--profile" ? args[++index] : arg.slice("--profile=".length);
      if (!value || value.startsWith("-")) throw new Error("--profile requires dev or release");
      if (!PROFILES.has(value)) throw new Error(`Unsupported profile: ${value}`);
      profile = value;
      continue;
    }
    if (arg === "--target" || arg.startsWith("--target=")) {
      const value = arg === "--target" ? args[++index] : arg.slice("--target=".length);
      if (!value || value.startsWith("-")) throw new Error("--target requires a Rust target triple");
      if (!TARGET_PATTERN.test(value)) throw new Error(`Invalid Rust target triple: ${value}`);
      if (target) throw new Error("--target may only be provided once");
      target = value;
      continue;
    }
    throw new Error(`Unknown argument: ${arg}`);
  }

  return { help, profile, target };
}

export function classifyTarget(target) {
  const components = target.split("-");
  if (components.length < 3 || !TARGET_PATTERN.test(target)) {
    throw new Error(`Invalid Rust target triple: ${target}`);
  }
  if (components.includes("darwin")) return "darwin";
  if (components.includes("linux")) return "linux";
  throw new Error(`Unsupported desktop target platform: ${target}`);
}

function normalizePlatform(platform) {
  if (platform === undefined || platform === "") return undefined;
  const normalized = { linux: "linux", darwin: "darwin", macos: "darwin" }[platform];
  if (!normalized) throw new Error(`Unsupported TAURI_ENV_PLATFORM: ${platform}`);
  return normalized;
}

function commandFailure(command, args, result) {
  const suffix = result?.signal ? ` (signal ${result.signal})` : ` (status ${result?.status ?? "unknown"})`;
  const error = new Error(`${command} ${args.join(" ")} failed${suffix}`);
  error.exitCode = Number.isInteger(result?.status) && result.status > 0 ? result.status : 1;
  return error;
}

function runCommand(command, args, options) {
  return spawnSync(command, args, options);
}

function rustcHost({ rootDir, run }) {
  const args = ["-vV"];
  const result = run("rustc", args, { cwd: rootDir, encoding: "utf8" });
  if (result?.error || result?.status !== 0 || result?.signal) throw commandFailure("rustc", args, result);
  const match = String(result.stdout ?? "").match(/^host:\s*(\S+)\s*$/m);
  if (!match) throw new Error("rustc -vV did not report a host target triple");
  return match[1];
}

export function resolveTarget({ requestedTarget, env = process.env, rootDir = REPO_ROOT, run = runCommand }) {
  const tauriTarget = env.TAURI_ENV_TARGET_TRIPLE?.trim() || undefined;
  if (requestedTarget && tauriTarget && requestedTarget !== tauriTarget) {
    throw new Error(`--target ${requestedTarget} does not match TAURI_ENV_TARGET_TRIPLE ${tauriTarget}`);
  }
  const target = requestedTarget || tauriTarget || rustcHost({ rootDir, run });
  const platform = classifyTarget(target);
  const tauriPlatform = normalizePlatform(env.TAURI_ENV_PLATFORM?.trim());
  if (tauriPlatform && tauriPlatform !== platform) {
    throw new Error(`Target ${target} is ${platform}, but TAURI_ENV_PLATFORM is ${tauriPlatform}`);
  }
  return { target, platform };
}

function targetRoot(rootDir, env) {
  return env.CARGO_TARGET_DIR ? resolve(rootDir, env.CARGO_TARGET_DIR) : join(rootDir, "target");
}

export function buildPlan({ profile, target, platform, rootDir = REPO_ROOT, env = process.env }) {
  if (!PROFILES.has(profile)) throw new Error(`Unsupported profile: ${profile}`);
  if (platform === "darwin") return { status: "noop", profile, target, platform, copies: [] };
  if (platform !== "linux") throw new Error(`Unsupported platform: ${platform}`);

  const fileName = ENGINE_BINARY_NAME;
  const cargoProfile = profile === "release" ? "release" : "debug";
  const output = join(targetRoot(rootDir, env), target, cargoProfile, fileName);
  const sidecar = join(rootDir, "apps", "desktop", "src-tauri", "binaries", `${ENGINE_BINARY_NAME}-${target}`);
  const copies = [sidecar];
  const devAdjacent = profile === "dev" ? join(targetRoot(rootDir, env), "debug", fileName) : undefined;
  if (devAdjacent) copies.push(devAdjacent);

  const commandArgs = [
    "build",
    "--locked",
    "--manifest-path",
    join(rootDir, "Cargo.toml"),
    "--package",
    "mixrelay-service",
  ];
  if (profile === "release") commandArgs.push("--release");
  commandArgs.push("--target", target);

  return {
    status: "build",
    profile,
    target,
    platform,
    command: ["cargo", ...commandArgs],
    output,
    sidecar,
    devAdjacent,
    copies,
  };
}

export function prepareDesktopEngine({
  args = [],
  env = process.env,
  rootDir = REPO_ROOT,
  fsApi = defaultFs,
  run = runCommand,
} = {}) {
  const parsed = parseArgs(args);
  if (parsed.help) return { status: "help", usage: usage() };
  const { target, platform } = resolveTarget({
    requestedTarget: parsed.target,
    env,
    rootDir,
    run,
  });
  const plan = buildPlan({ profile: parsed.profile, target, platform, rootDir, env });
  if (plan.status === "noop") return plan;

  const result = run(plan.command[0], plan.command.slice(1), { cwd: rootDir, stdio: "inherit", env });
  if (result?.error || result?.status !== 0 || result?.signal) {
    throw commandFailure(plan.command[0], plan.command.slice(1), result);
  }
  if (!fsApi.existsSync(plan.output)) throw new Error(`Cargo did not produce ${plan.output}`);
  for (const destination of plan.copies) {
    fsApi.mkdirSync(dirname(destination), { recursive: true });
    fsApi.copyFileSync(plan.output, destination);
  }
  return plan;
}

export function main(args = process.argv.slice(2), env = process.env) {
  try {
    const result = prepareDesktopEngine({ args, env });
    if (result.status === "help") console.log(result.usage);
    else if (result.status === "noop") console.log(`macOS target ${result.target}: existing Mach-service engine path preserved; no sidecar built`);
    else console.log(`Prepared ${result.profile} ${result.target} engine at ${result.sidecar}${result.devAdjacent ? ` and ${result.devAdjacent}` : ""}`);
    return 0;
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return Number.isInteger(error?.exitCode) ? error.exitCode : 1;
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  process.exitCode = main();
}
