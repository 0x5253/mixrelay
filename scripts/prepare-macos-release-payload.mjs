import { PRODUCT_DIRECTORY, APP_PATH, UNINSTALLER_PATH, REMOVAL_HELPER_PATH } from "./macos-install-contract.mjs";
import { createHash } from "node:crypto";
import { chmodSync, closeSync, copyFileSync, mkdirSync, openSync, readdirSync, readFileSync, lstatSync, readSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const DOCUMENTS = ["LICENSE.txt"];

export const EXPECTED_ARCHITECTURES = ["arm64", "x86_64"];
export const EXPECTED_MANAGER_IDENTIFIER = "io.mixrelay.desktop";
export const EXPECTED_HAL_IDENTIFIER = "io.mixrelay.audio.driver";
export const DRIVER_DESTINATION = "/Library/Application Support/MixRelay/bin/mixrelay-driver-service";
export const HAL_DESTINATION = "/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver";
export const MANAGER_DESTINATION = APP_PATH;
export const LAUNCHD_DESTINATION = "/Library/LaunchDaemons/io.mixrelay.audio.driver.plist";
export const LIFECYCLE_DIRECTORY = "/Library/Application Support/MixRelay/lifecycle";

const defaultFs = {
  chmodSync,
  closeSync,
  copyFileSync,
  lstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  readSync,
  writeFileSync,
};

const runCommand = (command, args, options) => spawnSync(command, args, options);

/**
 * Parse the explicit artifact paths accepted by the payload preparer.
 *
 * The command intentionally does not accept defaults: a release payload must
 * identify every executable input instead of silently selecting workspace
 * debug output.
 */
export function parseArgs(args) {
  const values = { manager: undefined, hal: undefined, driver: undefined, engine: undefined, uninstaller: undefined, output: undefined };
  let help = false;
  const options = new Set(Object.keys(values));

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--help" || arg === "-h") {
      help = true;
      continue;
    }
    const equals = arg.indexOf("=");
    const name = equals === -1 ? arg : arg.slice(0, equals);
    if (!name.startsWith("--") || !options.has(name.slice(2))) {
      throw new Error(`Unknown argument: ${arg}`);
    }
    const key = name.slice(2);
    const value = equals === -1 ? args[++index] : arg.slice(equals + 1);
    if (!value || value.startsWith("-")) throw new Error(`${name} requires an absolute path`);
    if (values[key] !== undefined) throw new Error(`${name} may only be provided once`);
    values[key] = value;
  }

  if (help) return { help: true, ...values };
  for (const key of options) {
    if (values[key] === undefined) throw new Error(`Missing required option: --${key}`);
    assertAbsolute(`--${key}`, values[key]);
  }
  return { help: false, ...values };
}

function usage() {
  return [
    "Usage: node scripts/prepare-macos-release-payload.mjs --manager MANAGER --hal HAL --driver DRIVER --engine ENGINE --uninstaller UNINSTALLER --output OUTPUT",
    "",
    "Preflights and stages an inspectable macOS release payload. It does not build, sign, notarize, or create a .pkg.",
  ].join("\n");
}

function assertAbsolute(name, value) {
  if (!value || !value.startsWith("/") || resolve(value) !== value) {
    throw new Error(`${name} must be an absolute, normalized path`);
  }
}

function assertNoSymlinkComponents(fsApi, name, path, allowMissingLeaf = false) {
  const components = path.split(sep).filter(Boolean);
  let current = sep;
  for (let index = 0; index < components.length; index += 1) {
    current = join(current, components[index]);
    let metadata;
    try {
      metadata = fsApi.lstatSync(current);
    } catch (error) {
      if (error.code === "ENOENT" && allowMissingLeaf) return;
      throw new Error(`Missing artifact path ${path}: ${error.message}`);
    }
    if (metadata.isSymbolicLink()) throw new Error(`${name} path must not contain symlinks: ${current}`);
    if (index < components.length - 1 && !metadata.isDirectory()) {
      throw new Error(`${name} path component must be a directory: ${current}`);
    }
  }
}

function rejectDevelopmentPath(name, value) {
  const normalized = value.split(sep).filter(Boolean);
  const hasCargoDebugPath = normalized.some((part, index) => {
    if (part === "target" && normalized[index + 1] === "debug") return true;
    if (part !== "target" || normalized[index + 2] !== "debug") return false;
    return /^[A-Za-z0-9_]+(?:-[A-Za-z0-9][A-Za-z0-9._-]*)+$/.test(normalized[index + 1]);
  });
  const hasUniversalDebugPath = normalized.some((part, index) => part === "universal" && normalized[index + 1] === "debug");
  if (hasCargoDebugPath || hasUniversalDebugPath || normalized.includes("macos-dev-audio")) {
    throw new Error(`${name} must not use workspace debug/development artifacts: ${value}`);
  }
}

function isWithin(path, parent) {
  return path === parent || path.startsWith(`${parent}${sep}`);
}

function assertDisjointArtifacts(output, artifacts) {
  for (const [name, path] of artifacts) {
    if (isWithin(output, path) || isWithin(path, output)) {
      throw new Error(`--output must be separate from ${name}: ${output}`);
    }
  }
  for (let leftIndex = 0; leftIndex < artifacts.length; leftIndex += 1) {
    const [leftName, leftPath] = artifacts[leftIndex];
    for (let rightIndex = leftIndex + 1; rightIndex < artifacts.length; rightIndex += 1) {
      const [rightName, rightPath] = artifacts[rightIndex];
      if (isWithin(leftPath, rightPath) || isWithin(rightPath, leftPath)) {
        throw new Error(`${leftName} must be separate from ${rightName}: ${leftPath}`);
      }
    }
  }
}

function stat(fsApi, path) {
  try {
    return fsApi.lstatSync(path);
  } catch (error) {
    throw new Error(`Missing artifact path ${path}: ${error.message}`);
  }
}

function assertDirectory(fsApi, name, path) {
  assertNoSymlinkComponents(fsApi, name, path);
  const metadata = stat(fsApi, path);
  if (metadata.isSymbolicLink()) throw new Error(`${name} must not be a symlink: ${path}`);
  if (!metadata.isDirectory()) throw new Error(`${name} must be a directory: ${path}`);
}

function assertExecutable(fsApi, name, path) {
  assertNoSymlinkComponents(fsApi, name, path);
  const metadata = stat(fsApi, path);
  if (metadata.isSymbolicLink()) throw new Error(`${name} must not be a symlink: ${path}`);
  if (!metadata.isFile()) throw new Error(`${name} must be a regular file: ${path}`);
  if (metadata.size === 0 || (metadata.mode & 0o111) === 0) {
    throw new Error(`${name} must be a non-empty executable: ${path}`);
  }
}

function assertRegularFile(fsApi, name, path) {
  assertNoSymlinkComponents(fsApi, name, path);
  const metadata = stat(fsApi, path);
  if (metadata.isSymbolicLink()) throw new Error(`${name} must not be a symlink: ${path}`);
  if (!metadata.isFile()) throw new Error(`${name} must be a regular file: ${path}`);
}

function runChecked(run, command, args, description) {
  const result = run(command, args, {
    encoding: "utf8",
    maxBuffer: 1024 * 1024,
    timeout: 30_000,
  });
  if (result?.error || result?.status !== 0 || result?.signal) {
    const detail = result?.error?.message ?? result?.stderr ?? result?.signal ?? `status ${result?.status}`;
    throw new Error(`${description} failed: ${detail}`);
  }
  return String(result.stdout ?? "").trim();
}

function plistValue(run, path, key) {
  return runChecked(
    run,
    "/usr/bin/plutil",
    ["-extract", key, "raw", "-o", "-", path],
    `read ${key} from ${path}`,
  );
}

function validateIdentifier(run, path, key, expected, name) {
  if (plistValue(run, path, key) !== expected) {
    throw new Error(`${name} has unexpected ${key}; expected ${expected}: ${path}`);
  }
}

function universalArchitectures(run, path) {
  const description = runChecked(run, "/usr/bin/file", ["-b", path], `inspect Mach-O ${path}`);
  if (!description.includes("Mach-O")) throw new Error(`Expected Mach-O executable: ${path}`);
  const actual = runChecked(run, "/usr/bin/xcrun", ["lipo", "-archs", path], `inspect architectures ${path}`)
    .split(/\s+/)
    .filter(Boolean)
    .sort();
  const expected = [...EXPECTED_ARCHITECTURES].sort();
  if (actual.length !== expected.length || actual.some((architecture, index) => architecture !== expected[index])) {
    throw new Error(`Expected exactly arm64 and x86_64 in ${path}; found ${actual.join(" ") || "none"}`);
  }
  return [...EXPECTED_ARCHITECTURES];
}

function walkFiles(fsApi, root) {
  const entries = fsApi.readdirSync(root, { withFileTypes: true });
  const files = [];
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
    const path = join(root, entry.name);
    if (entry.isSymbolicLink()) throw new Error(`Artifact tree must not contain symlinks: ${path}`);
    if (entry.isDirectory()) files.push(...walkFiles(fsApi, path));
    else if (entry.isFile()) files.push(path);
    else throw new Error(`Artifact tree contains unsupported inode: ${path}`);
  }
  return files;
}

function validateBundleTree(fsApi, run, name, bundle, executableRelativePath, identifier) {
  assertDirectory(fsApi, name, bundle);
  const infoPlist = join(bundle, "Contents", "Info.plist");
  assertRegularFile(fsApi, `${name} Info.plist`, infoPlist);
  validateIdentifier(run, infoPlist, "CFBundleIdentifier", identifier, name);
  const executable = join(bundle, executableRelativePath);
  assertExecutable(fsApi, `${name} executable`, executable);

  const binaries = [];
  for (const path of walkFiles(fsApi, bundle)) {
    const description = runChecked(run, "/usr/bin/file", ["-b", path], `inspect ${name} payload ${path}`);
    if (description.includes("Mach-O")) binaries.push(path);
  }
  if (!binaries.includes(executable)) binaries.push(executable);
  for (const path of [...new Set(binaries)].sort()) universalArchitectures(run, path);
  return { identifier, executable, binaries: [...new Set(binaries)].sort() };
}

function assertOutput(fsApi, output) {
  assertAbsolute("--output", output);
  assertNoSymlinkComponents(fsApi, "--output", output, true);
  try {
    const metadata = fsApi.lstatSync(output);
    if (metadata.isSymbolicLink()) throw new Error(`--output must not be a symlink: ${output}`);
    if (!metadata.isDirectory()) throw new Error(`--output must be a directory: ${output}`);
    if (fsApi.readdirSync(output).length !== 0) throw new Error(`--output must be empty: ${output}`);
  } catch (error) {
    if (error.code === "ENOENT") return;
    throw error;
  }
}

function ensureDirectories(fsApi, root, directory) {
  if (!isWithin(directory, root)) throw new Error(`staging directory must remain under --output: ${directory}`);
  if (directory === root) return;
  let current = root;
  for (const component of relative(root, directory).split(sep)) {
    current = join(current, component);
    try {
      fsApi.mkdirSync(current, { mode: 0o755 });
    } catch (error) {
      if (error.code !== "EEXIST") throw error;
    }
    const metadata = stat(fsApi, current);
    if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
      throw new Error(`staging path is not a directory: ${current}`);
    }
    fsApi.chmodSync(current, 0o755);
  }
}

function copyTree(fsApi, source, destination, stagingRoot) {
  const metadata = stat(fsApi, source);
  if (metadata.isSymbolicLink()) throw new Error(`Refusing to stage symlink: ${source}`);
  if (metadata.isDirectory()) {
    ensureDirectories(fsApi, stagingRoot, destination);
    const entries = fsApi.readdirSync(source, { withFileTypes: true });
    for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
      const path = join(source, entry.name);
      const target = join(destination, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`Artifact tree must not contain symlinks: ${path}`);
      if (entry.isDirectory()) copyTree(fsApi, path, target, stagingRoot);
      else if (entry.isFile()) {
        ensureDirectories(fsApi, stagingRoot, dirname(target));
        fsApi.copyFileSync(path, target);
      } else throw new Error(`Artifact tree contains unsupported inode: ${path}`);
    }
    return;
  }
  ensureDirectories(fsApi, stagingRoot, dirname(destination));
  fsApi.copyFileSync(source, destination);
}

function launchdTemplate() {
  return readFileSync(new URL("../packaging/macos/io.mixrelay.audio.driver.plist.in", import.meta.url), "utf8");
}

function sha256(fsApi, path) {
  const descriptor = fsApi.openSync(path, "r");
  const hash = createHash("sha256");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    let bytesRead;
    do {
      bytesRead = fsApi.readSync(descriptor, buffer, 0, buffer.length, null);
      if (bytesRead > 0) hash.update(buffer.subarray(0, bytesRead));
    } while (bytesRead > 0);
    return hash.digest("hex");
  } finally {
    fsApi.closeSync(descriptor);
  }
}

function treeManifest(fsApi, source) {
  return walkFiles(fsApi, source).map(path => ({
    path: relative(source, path).split(sep).join("/"),
    sha256: sha256(fsApi, path),
  }));
}

/**
 * Validate and stage release artifacts without invoking pkgbuild, productbuild,
 * installer, launchctl, codesign, or any privileged operation.
 */
export function preparePayload({ manager, hal, driver, engine, uninstaller, output, fsApi = defaultFs, run = runCommand }) {
  for (const [name, path] of [["--manager", manager], ["--hal", hal], ["--driver", driver], ["--engine", engine], ["--uninstaller", uninstaller], ["--output", output]]) {
    assertAbsolute(name, path);
    rejectDevelopmentPath(name, path);
  }
  assertNoSymlinkComponents(fsApi, "--manager", manager);
  assertNoSymlinkComponents(fsApi, "--hal", hal);
  assertNoSymlinkComponents(fsApi, "--driver", driver);
  assertNoSymlinkComponents(fsApi, "--output", output, true);
  assertDisjointArtifacts(output, [["manager", manager], ["HAL", hal], ["driver", driver], ["engine", engine], ["uninstaller", uninstaller]]);
  assertOutput(fsApi, output);
  const managerInfo = validateBundleTree(fsApi, run, "manager", manager, "Contents/MacOS/mixrelay-desktop", EXPECTED_MANAGER_IDENTIFIER);
  for (const filename of DOCUMENTS) {
    const document = join(manager, "Contents/Resources", filename);
    assertRegularFile(fsApi, `manager ${filename}`, document);
    if (stat(fsApi, document).size === 0) throw new Error(`manager ${filename} must be non-empty: ${document}`);
  }
  const uninstallerInfo = validateBundleTree(fsApi, run, "uninstaller", uninstaller, "Contents/MacOS/mixrelay-driver-uninstaller", "io.mixrelay.driver.uninstaller");
  const team = plistValue(run, join(uninstaller, "Contents/Info.plist"), "MixRelaySigningTeam");
  if (!/^[A-Z0-9]{10}$/.test(team)) throw new Error("uninstaller publisher identity is invalid");
  assertExecutable(fsApi, "removal helper", join(uninstaller, "Contents/Helpers/mixrelay-macos-installer"));
  const halInfo = validateBundleTree(fsApi, run, "HAL", hal, "Contents/MacOS/MixRelayHAL", EXPECTED_HAL_IDENTIFIER);
  assertExecutable(fsApi, "driver", driver);
  assertExecutable(fsApi, "engine", engine);
  universalArchitectures(run, engine);
  const driverArchitectures = universalArchitectures(run, driver);

  const managerTarget = join(output, APP_PATH.slice(1));
  const halTarget = join(output, "Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver");
  const driverTarget = join(output, "Library/Application Support/MixRelay/bin/mixrelay-driver-service");
  const plistTarget = join(output, "templates/io.mixrelay.audio.driver.plist.in");
  fsApi.mkdirSync(output, { recursive: true, mode: 0o755 });
  copyTree(fsApi, manager, managerTarget, output);
  copyTree(fsApi, uninstaller, join(output, UNINSTALLER_PATH.slice(1)), output);
  copyTree(fsApi, hal, halTarget, output);
  copyTree(fsApi, driver, driverTarget, output);
  copyTree(fsApi, engine, join(output, "Library/Application Support/MixRelay/bin/mixrelay-service"), output);
  ensureDirectories(fsApi, output, dirname(plistTarget));
  fsApi.writeFileSync(plistTarget, launchdTemplate(), "utf8");
  fsApi.writeFileSync(join(output, "templates/io.mixrelay.audio.engine.plist.in"), readFileSync(new URL("../packaging/macos/io.mixrelay.audio.engine.plist.in", import.meta.url), "utf8"), "utf8");
  fsApi.writeFileSync(join(output, "templates/io.mixrelay.driver.removal.plist"), removalLaunchdTemplate(team), "utf8");

  const manifest = {
    format: 1,
    status: "payload-prepared-not-package",
    installContract: {
      productDirectory: PRODUCT_DIRECTORY,
      manager: MANAGER_DESTINATION,
      driver: DRIVER_DESTINATION,
      engine: "/Library/Application Support/MixRelay/bin/mixrelay-service",
      engineLaunchd: "/Library/LaunchDaemons/io.mixrelay.audio.engine.plist",
      hal: HAL_DESTINATION,
      launchd: LAUNCHD_DESTINATION,
      uninstaller: UNINSTALLER_PATH,
      removalLaunchd: "/Library/LaunchDaemons/io.mixrelay.driver.removal.plist",
      destinations: [
        { path: PRODUCT_DIRECTORY, owner: "root:wheel", mode: "0755" },
        { path: UNINSTALLER_PATH, owner: "root:wheel", mode: "0755" },
        { path: MANAGER_DESTINATION, owner: "root:wheel", mode: "0755" },
        { path: DRIVER_DESTINATION, owner: "root:wheel", mode: "0755" },
        { path: "/Library/Application Support/MixRelay/bin/mixrelay-service", owner: "root:wheel", mode: "0755" },
        { path: "/Library/LaunchDaemons/io.mixrelay.audio.engine.plist", owner: "root:wheel", mode: "0644" },
        { path: HAL_DESTINATION, owner: "root:wheel", modePolicy: "not-group-or-other-writable" },
        { path: LAUNCHD_DESTINATION, owner: "root:wheel", mode: "0644" },
        { path: "/Library/Application Support/MixRelay", owner: "root:wheel", mode: "0755" },
        { path: LIFECYCLE_DIRECTORY, owner: "root:wheel", mode: "0755" },
        { path: `${LIFECYCLE_DIRECTORY}/app.owner`, owner: "<service-user>:<primary-group>", mode: "0644" },
        { path: `${LIFECYCLE_DIRECTORY}/engine.owner`, owner: "<service-user>:<primary-group>", mode: "0644" },
        { path: `${LIFECYCLE_DIRECTORY}/installer.owner`, owner: "root:wheel", mode: "0644" },
      ],
      requiresPostinstallUserExpansion: true,
      requiresPrivilegedInstaller: true,
    },
    artifacts: [
      { kind: "manager", source: manager, target: MANAGER_DESTINATION, identifier: managerInfo.identifier, architectures: EXPECTED_ARCHITECTURES, files: treeManifest(fsApi, manager) },
      { kind: "uninstaller", source: uninstaller, target: UNINSTALLER_PATH, identifier: uninstallerInfo.identifier, architectures: EXPECTED_ARCHITECTURES, files: treeManifest(fsApi, uninstaller) },
      { kind: "removal-launchd", target: "/Library/LaunchDaemons/io.mixrelay.driver.removal.plist", template: "templates/io.mixrelay.driver.removal.plist" },
      { kind: "hal", source: hal, target: HAL_DESTINATION, identifier: halInfo.identifier, architectures: EXPECTED_ARCHITECTURES, files: treeManifest(fsApi, hal) },
      { kind: "driver", source: driver, target: DRIVER_DESTINATION, architectures: driverArchitectures, sha256: sha256(fsApi, driver) },
      { kind: "engine", source: engine, target: "/Library/Application Support/MixRelay/bin/mixrelay-service", architectures: EXPECTED_ARCHITECTURES, sha256: sha256(fsApi, engine) },
      { kind: "engine-template", target: "/Library/LaunchDaemons/io.mixrelay.audio.engine.plist", template: "templates/io.mixrelay.audio.engine.plist.in" },
      { kind: "launchd-template", target: LAUNCHD_DESTINATION, template: "templates/io.mixrelay.audio.driver.plist.in" },
    ],
    limitations: [
      "No pkg was generated.",
      "No installer or uninstaller scripts are included.",
      "Code signing, notarization, launchd bootstrap, CoreAudio reload, and live HAL verification remain outstanding.",
    ],
  };
  fsApi.writeFileSync(join(output, "payload-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  return { ...manifest, output };
}

export function main(args = process.argv.slice(2)) {
  try {
    const parsed = parseArgs(args);
    if (parsed.help) {
      console.log(usage());
      return 0;
    }
    if (process.platform !== "darwin") throw new Error("macOS release payload preparation must run on macOS");
    const result = preparePayload(parsed);
    console.log(`Prepared inspectable payload at ${result.output}; no .pkg was generated`);
    return 0;
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exitCode = main();
}

export function removalLaunchdTemplate(team) {
  if (!/^[A-Z0-9]{10}$/.test(team)) throw new Error("Invalid removal helper team");
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>io.mixrelay.driver.removal</string>
<key>ProgramArguments</key><array><string>${REMOVAL_HELPER_PATH}</string><string>removal-helper</string><string>${team}</string></array>
<key>MachServices</key><dict><key>io.mixrelay.driver.removal</key><true/></dict>
<key>AssociatedBundleIdentifiers</key><array><string>io.mixrelay.driver.uninstaller</string></array>
</dict></plist>
`;
}
