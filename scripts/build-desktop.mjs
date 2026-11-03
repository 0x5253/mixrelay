import { assertDesktopPackagingAllowed } from './core/desktop-build-policy.mjs';
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { inspectPackage } from "./check-linux-package.mjs";
import { loadContract, checkGenerated } from './support-contract.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const desktopRoot = resolve(root, "apps/desktop-shell");
const require = createRequire(import.meta.url);
const LINUX_BUNDLE_FORMATS = new Set(["deb", "rpm"]);
export function desktopBuildCommand(platform, args = []) {
  if (platform === "darwin") {
    if (args.length !== 0) {
      throw new Error("macOS desktop:build produces a universal release app and accepts no overrides");
    }
    return ["zsh", "scripts/build-macos-universal-app.sh", ];
  }
  if (platform === "linux") {
    const cli = require.resolve("@tauri-apps/cli/tauri.js", { paths: [desktopRoot] });
    return [process.execPath, cli, "build", ...args];
  }
  throw new Error(`Unsupported desktop build host: ${platform}`);
}

function bundleArguments(args) {
  const selected = [];
  let explicit = false;
  const appendValues = value => selected.push(...value.split(",").filter(Boolean));
  const appendFollowingValues = index => {
    while (index + 1 < args.length && !args[index + 1].startsWith("-")) {
      index += 1;
      appendValues(args[index]);
    }
    return index;
  };

  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--") break;
    if (argument === "--no-bundle") return { explicit: true, selected: [] };
    if (argument === "-b" || argument === "--bundles") {
      explicit = true;
      index = appendFollowingValues(index);
    } else if (argument.startsWith("--bundles=")) {
      explicit = true;
      appendValues(argument.slice("--bundles=".length));
      index = appendFollowingValues(index);
    } else if (argument.startsWith("-b") && argument.length > 2) {
      explicit = true;
      appendValues(argument.slice(2).replace(/^=/, ""));
      index = appendFollowingValues(index);
    }
  }
  return { explicit, selected };
}

function hasLinuxConfigOverride(args) {
  for (const argument of args) {
    if (argument === "--") return false;
    if (argument === "--config" || argument.startsWith("--config=")
      || argument === "-c" || (argument.startsWith("-c") && argument.length > 2)) {
      return true;
    }
  }
  return false;
}

function hasExplicitLinuxBundleAction(args) {
  for (const argument of args) {
    if (argument === "--") break;
    if (argument === "--no-bundle") return true;
  }
  const { explicit, selected } = bundleArguments(args);
  return explicit && selected.length > 0 && selected.every(format => LINUX_BUNDLE_FORMATS.has(format));
}

function validateLinuxConfigBuildArguments(args) {
  const { selected } = bundleArguments(args);
  if (selected.some(format => !LINUX_BUNDLE_FORMATS.has(format))) {
    throw new Error("Linux release packages must use deb or rpm");
  }
  if (hasLinuxConfigOverride(args) && !hasExplicitLinuxBundleAction(args)) {
    throw new Error("Linux desktop builds with --config/-c must provide a valid --bundles/-b selection or --no-bundle");
  }
}

export function linuxBuildIncludesDebian(args = []) {
  const { explicit, selected } = bundleArguments(args);
  // The Linux config defaults to deb and rpm.
  return explicit ? selected.includes("deb") : true;
}

function optionValue(args, longName, shortName) {
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--") break;
    if (argument === longName || argument === shortName) return args[index + 1];
    if (argument.startsWith(`${longName}=`)) return argument.slice(longName.length + 1);
    if (argument.startsWith(`${shortName}=`)) return argument.slice(shortName.length + 1);
    if (argument.startsWith(shortName) && argument.length > shortName.length) {
      return argument.slice(shortName.length).replace(/^=/, "");
    }
  }
  return undefined;
}

export function linuxDebianBundleDirectory(args = [], env = process.env) {
  const targetRoot = env.CARGO_TARGET_DIR
    ? resolve(root, env.CARGO_TARGET_DIR)
    : resolve(root, "target");
  const target = optionValue(args, "--target", "-t");
  const separator = args.indexOf("--");
  const buildArgs = separator === -1 ? args : args.slice(0, separator);
  const profile = buildArgs.some(argument => argument === "--debug" || argument === "-d") ? "debug" : "release";
  return resolve(targetRoot, ...(target ? [target] : []), profile, "bundle", "deb");
}

export function listDebianPackages(directory, listDirectory = readdirSync) {
  let entries;
  try {
    entries = listDirectory(directory, { withFileTypes: true });
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
  const packages = [];
  for (const entry of entries) {
    if (!entry.name.endsWith(".deb")) continue;
    if (!entry.isFile()) throw new Error(`Debian build artifact is not a regular file: ${entry.name}`);
    packages.push(resolve(directory, entry.name));
  }
  return packages.sort();
}

export async function main(
  args = process.argv.slice(2),
  platform = process.platform,
  run = spawnSync,
  inspectDebianPackage = inspectPackage,
  findDebianPackages = listDebianPackages,
  env = process.env,
) {
  try {
    const buildEnv = { ...env };
    assertDesktopPackagingAllowed(buildEnv);
    if (platform === "linux") validateLinuxConfigBuildArguments(args);
    const [command, ...commandArgs] = desktopBuildCommand(platform, args);
    // Tauri and its engine hook run from different directories. Resolve once
    // at the repository boundary so both build and inspect the same target tree.
    if (platform !== "darwin" && buildEnv.CARGO_TARGET_DIR) {
      buildEnv.CARGO_TARGET_DIR = resolve(root, buildEnv.CARGO_TARGET_DIR);
    }
    // `run` may be the signing wrapper's asynchronous owned runner (see macos-signing.mjs).
    const result = await run(command, commandArgs, {
      cwd: platform === "darwin" ? root : desktopRoot,
      stdio: "inherit",
      env: buildEnv,
    });
    if (result.error) throw result.error;
    if (result.signal) throw new Error(`Desktop build interrupted by ${result.signal}`);
    const status = result.status ?? 1;
    if (status !== 0 || platform !== "linux" || !linuxBuildIncludesDebian(args)) return status;

    const bundleDirectory = linuxDebianBundleDirectory(args, buildEnv);
    const packages = findDebianPackages(bundleDirectory);
    if (packages.length !== 1) {
      throw new Error(`Expected exactly one Debian package in ${bundleDirectory}; found ${packages.length}`);
    }
    const inspected = await inspectDebianPackage(packages[0]);
    if (inspected && inspected.packageName !== "mix-relay") {
      throw new Error("The product build must produce the complete mix-relay package");
    }
    return 0;
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return 1;
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const build = async () => {
    checkGenerated(loadContract().contract);
    if (process.platform !== "darwin") return main();
    desktopBuildCommand("darwin", process.argv.slice(2));
    const { checkMacosSigning, withMacosSigning } = await import("./macos-signing.mjs");
    checkMacosSigning();
    return withMacosSigning((env, _config, owned) => main(process.argv.slice(2), "darwin", owned, inspectPackage, listDebianPackages, env));
  };
  build().then(status => { process.exitCode = status; }).catch(error => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
