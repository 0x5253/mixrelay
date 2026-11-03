import assert from "node:assert/strict";
import * as fs from "node:fs";
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync, chmodSync, readdirSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { buildPackages, checkPackageOutput, parseArgs, installerRtf } from "./build-macos-installer-pkgs.mjs";
import { fixedComponentBundlePolicy } from "./core/macos-package-policy.mjs";

const TEAM = "AB12C3D4E5";
const IDENTITY = `Developer ID Installer: MixRelay Release (${TEAM})`;
const LANGUAGES = JSON.parse(readFileSync(new URL("../packaging/localization/languages.json", import.meta.url), "utf8"));
const OPERATIONS = JSON.parse(readFileSync(new URL("../packaging/localization/installer-operations.json", import.meta.url), "utf8"));
const DOCUMENTS = ["LICENSE.txt"];
const SOURCE_DOCUMENTS = Object.fromEntries(DOCUMENTS.map(filename => [filename, readFileSync(new URL(`../${filename}`, import.meta.url))]));
const LOCALIZED_NOTICES = JSON.parse(readFileSync(new URL("../packaging/localization/installer-notices.json", import.meta.url), "utf8"));

function rtfText(resource) {
  const rtf = resource.toString("utf8").replace(/\{\\fonttbl\{[^{}]*\}\}/, "");
  return [...rtf.matchAll(/\\([\\{}])|\\u(-?\d+)\?|\\([a-z]+)(-?\d+)? ?|[{}]|\r?\n|([^\\{}\r\n]+)/g)]
    .map(match => {
      if (match[1]) return match[1];
      if (match[2] !== undefined) return String.fromCharCode(Number(match[2]) & 0xffff);
      if (match[3]) return ["par", "line"].includes(match[3]) ? "\n" : match[3] === "tab" ? "\t" : "";
      return match[5] ?? "";
    }).join("");
}

function presentationWords(text) {
  return text.replace(/\s+/gu, " ").trim();
}

function assertInstallerTypography(resource) {
  const rtf = resource.toString("utf8");
  assert.match(rtf, /^\{\\rtf1\\ansi\\deff0\\uc1\{\\fonttbl\{\\f0\\fswiss Helvetica;\}\}/);
  assert.match(rtf, /\\pard\\ql\\f0\\fs28\\b\\sb0\\sa80 /);
  assert.match(rtf, /\\pard\\ql\\f0\\fs22\\b0\\sl280\\slmult0\\sb0\\sa120 /);
  assert.doesNotMatch(rtf, /\\(?:colortbl|cf\d|cb\d|highlight|paperw|margl|margr)/);
}

function assertCopyrightNotice(resources, plainNotice) {
  assert.deepEqual(plainNotice, SOURCE_DOCUMENTS["LICENSE.txt"]);
  assert.ok(Object.keys(resources).every(path => !/LICENSE/.test(path)));
}

function assertInstallerReadme(resources) {
  const readme = resources["en.lproj/INSTALLER-README.rtf"];
  assertInstallerTypography(readme);
  const notice = rtfText(readme);
  assert.match(notice, /^MixRelay\nInstallation Guide\n/);
  assert.match(notice, /installs the audio editor, virtual audio devices and background audio services together/);
  assert.match(notice, /Before installation\n/);
  assert.match(notice, /After installation\n/);
  assert.match(notice, /Quitting the MixRelay app alone does not stop the Driver/);
  assert.match(notice, /Saved settings are also preserved when updating/);
  assert.match(notice, /Uninstall MixRelay/);
  assert.doesNotMatch(notice, /success|Terms of Use|Changelog|Unreleased|Release notes|Release fixture/i);
  assert.equal(resources["INSTALLER-README.rtf"], undefined, "unlocalized guidance must not shadow the preferred language");
  for (const entry of LANGUAGES.filter(({ language }) => !["en", "ko"].includes(language))) {
    const document = resources[`${entry.language}.lproj/INSTALLER-README.rtf`];
    assertInstallerTypography(document);
    assert.equal(presentationWords(rtfText(document)), presentationWords(LOCALIZED_NOTICES[entry.language]));
    assert.ok(rtfText(document).includes(entry.guideTitle));
    assert.ok(rtfText(document).includes(entry.beforeHeading));
    assert.ok(rtfText(document).includes(entry.afterHeading));
  }
  const korean = resources["ko.lproj/INSTALLER-README.rtf"];
  assertInstallerTypography(korean);
  assert.match(rtfText(korean), /^MixRelay\n설치 안내\n/);
  assert.match(rtfText(korean), /시스템 전체의 오디오가 잠시 중단됩니다/);
  assert.match(rtfText(korean), /Mac을 재부팅할 필요는 없습니다/);
  assert.match(rtfText(korean), /MixRelay 상태 메뉴에서 종료를 선택하면 모든 Driver 장치와 오디오 전달이 중지되며, 장치 설정은 유지됩니다/);
  assert.match(rtfText(korean), /저장된 설정으로 장치와 오디오 경로를 다시 시작합니다/);
  assert.match(rtfText(korean), /MixRelay 앱을 종료하는 것만으로는 Driver가 중지되지 않습니다/);
}

function fixture(t) {
  const fixtureRoot = fileURLToPath(new URL("../target/test-artifacts/", import.meta.url));
  mkdirSync(fixtureRoot, { recursive: true });
  const root = mkdtempSync(join(fixtureRoot, "mixrelay-macos-pkg-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const payload = join(root, "prepared payload");
  const manager = join(payload, "Applications/MixRelay/MixRelay.app");
  const uninstaller = join(payload, "Applications/MixRelay/Uninstall MixRelay.app");
  const hal = join(payload, "Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver");
  const driver = join(payload, "Library/Application Support/MixRelay/bin/mixrelay-driver-service");
  const template = join(payload, "templates/io.mixrelay.audio.driver.plist.in");
  const cli = join(root, "mixrelay-macos-installer");
  const payloadChecker = join(root, "mixrelay-macos-payload-check");
  for (const [bundle, identifier, packageType] of [
    [manager, "io.mixrelay.desktop", "APPL"],
    [uninstaller, "io.mixrelay.driver.uninstaller", "APPL"],
    [hal, "io.mixrelay.audio.MixRelayHAL", "BNDL"],
  ]) {
    mkdirSync(join(bundle, "Contents/MacOS"), { recursive: true });
    writeFileSync(join(bundle, "Contents/MacOS/main"), "signed mach-o fixture");
    writeFileSync(join(bundle, "Contents/Info.plist"), `<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${identifier}</string><key>CFBundleName</key><string>MixRelay</string><key>CFBundlePackageType</key><string>${packageType}</string><key>CFBundleVersion</key><string>1.2.3</string></dict></plist>`);
  }
  const nested = join(manager, "Contents/PlugIns/MixRelayEditor.appex");
  mkdirSync(join(manager, "Contents/Resources"), { recursive: true });
  writeFileSync(join(manager, "Contents/Resources/LICENSE.txt"), SOURCE_DOCUMENTS["LICENSE.txt"]);
  mkdirSync(join(nested, "Contents/MacOS"), { recursive: true });
  for (const bundle of [manager, hal, nested]) chmodSync(bundle, 0o755);
  writeFileSync(join(nested, "Contents/MacOS/editor"), "nested editor fixture");
  writeFileSync(join(nested, "Contents/Info.plist"), "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>io.mixrelay.desktop.editor</string><key>CFBundleName</key><string>MixRelayEditor</string><key>CFBundlePackageType</key><string>XPC!</string><key>CFBundleVersion</key><string>1.2.3</string></dict></plist>");
  mkdirSync(join(payload, "Library/Application Support/MixRelay/bin"), { recursive: true });
  mkdirSync(join(payload, "templates"), { recursive: true });
  writeFileSync(driver, "universal driver fixture");
  const engine = join(payload, "Library/Application Support/MixRelay/bin/mixrelay-service");
  writeFileSync(engine, "universal engine fixture");
  chmodSync(engine, 0o755);
  writeFileSync(join(payload, "templates/io.mixrelay.audio.engine.plist.in"), "engine template fixture");
  writeFileSync(template, "launchd template fixture");
  writeFileSync(join(payload, "templates/io.mixrelay.driver.removal.plist"), "removal plist fixture");
  writeFileSync(cli, "universal cli fixture");
  writeFileSync(payloadChecker, "payload checker fixture");
  chmodSync(driver, 0o755);
  chmodSync(cli, 0o755);
  chmodSync(payloadChecker, 0o755);
  writeFileSync(join(payload, "payload-manifest.json"), JSON.stringify({
    format: 1,
    status: "payload-prepared-not-package",
    installContract: {
      productDirectory: "/Applications/MixRelay",
      manager: "/Applications/MixRelay/MixRelay.app",
      uninstaller: "/Applications/MixRelay/Uninstall MixRelay.app",
      removalLaunchd: "/Library/LaunchDaemons/io.mixrelay.driver.removal.plist",
      engine: "/Library/Application Support/MixRelay/bin/mixrelay-service",
      engineLaunchd: "/Library/LaunchDaemons/io.mixrelay.audio.engine.plist",
      hal: "/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver",
      driver: "/Library/Application Support/MixRelay/bin/mixrelay-driver-service",
      launchd: "/Library/LaunchDaemons/io.mixrelay.audio.driver.plist",
    },
  }));
  return { root, payload, cli, installerCli: cli, payloadChecker, output: join(root, "packages"), skipNotarization: true };
}

function packageSignature(identity = IDENTITY, status = "signed by a developer certificate issued by Apple for distribution") {
  return `Package "fixture.pkg":\n   Status: ${status}\n   Signed with a trusted timestamp on: 2026-10-05 21:06:51 +0000\n   Certificate Chain:\n    1. ${identity}\n    2. Developer ID Certification Authority\n    3. Apple Root CA\n`;
}

function fakeRunner(calls, signature = IDENTITY, inspectPackageRoot = () => {}, payloadCheck = { status: 0, stdout: "payload signatures verified" }, checkScriptSyntax = true) {
  const analyzedPolicies = new Map();
  return (command, args, options) => {
    const call = { command, args };
    calls.push(call);
    if (command.endsWith("mixrelay-macos-payload-check")) {
      call.environment = options.env;
      return payloadCheck;
    }
    if (args[0] === "--version") return { status: 0, stdout: "1.2.3" };
    if (command === "/usr/bin/file") return { status: 0, stdout: "Mach-O 64-bit executable" };
    if (command === "/usr/bin/xcrun") return { status: 0, stdout: "arm64 x86_64" };
    if (command === "/usr/bin/codesign") return { status: 0, stdout: "" };
    if (command === "/usr/bin/ditto") {
      fs.cpSync(args[0], args[1], { recursive: true, preserveTimestamps: true });
      return { status: 0, stdout: "" };
    }
    if (command === "/usr/bin/plutil") {
      if (args[0] === "-extract" && args[1] === "CFBundleShortVersionString") return { status: 0, stdout: "1.2.3" };
      if (args[0] === "-extract") {
        const plist = readFileSync(args.at(-1), "utf8");
        return { status: 0, stdout: plist.match(/<key>MixRelaySigningTeam<\/key><string>([^<]+)<\/string>/)?.[1] ?? "" };
      }
      if (args[1] === "json") {
        return { status: 0, stdout: JSON.stringify(analyzedPolicies.get(args.at(-1)) ?? []) };
      }
      if (args[1] === "xml1") {
        const policy = JSON.parse(readFileSync(args.at(-1), "utf8"));
        const output = args[args.indexOf("-o") + 1];
        analyzedPolicies.set(output, policy);
        writeFileSync(output, "fixture plist");
        return { status: 0, stdout: "" };
      }
      throw new Error("unexpected plutil invocation");
    }
    if (command === "/usr/bin/pkgbuild" || command === "/usr/bin/productbuild") {
      if (command === "/usr/bin/pkgbuild") {
        if (args.includes("--nopayload")) {
          const scripts = args[args.indexOf("--scripts") + 1];
          call.postinstall = readFileSync(join(scripts, "postinstall"), "utf8");
          call.embeddedCli = readFileSync(join(scripts, "mixrelay-macos-installer"));
          if (checkScriptSyntax) assert.equal(spawnSync("/bin/sh", ["-n", join(scripts, "postinstall")]).status, 0);
          writeFileSync(args.at(-1), "scripts-only fixture");
          return { status: 0, stdout: "" };
        }
        const root = args[args.indexOf("--root") + 1];
        if (args[0] === "--analyze") {
          inspectPackageRoot(root);
          const bundleDirectories = [];
          const walkDirectories = directory => {
            for (const entry of readdirSync(directory, { withFileTypes: true })) {
              if (!entry.isDirectory()) continue;
              const child = join(directory, entry.name);
              if ([".app", ".driver", ".appex", ".framework", ".bundle", ".xpc"].some(extension => child.endsWith(extension))) {
                bundleDirectories.push(child);
              }
              walkDirectories(child);
            }
          };
          walkDirectories(root);
          const sorted = bundleDirectories.sort((left, right) => left.length - right.length || left.localeCompare(right));
          const entries = [];
          for (const path of sorted) {
            const parent = sorted
              .filter(candidate => candidate !== path && path.startsWith(`${candidate}/`))
              .sort((left, right) => right.length - left.length)[0];
            const value = { RootRelativeBundlePath: relative(root, path).split("/").join("/"), ChildBundles: [] };
            if (parent) {
              const owner = [...entries].reverse().find(entry => entry.RootRelativeBundlePath === relative(root, parent).split("/").join("/"));
              owner.ChildBundles.push(value);
            } else {
              entries.push(value);
            }
          }
          const analysisPath = args.at(-1);
          analyzedPolicies.set(analysisPath, entries);
          writeFileSync(analysisPath, "fixture analysis plist");
          return { status: 0, stdout: "" };
        }
        const walk = path => readdirSync(path, { withFileTypes: true }).flatMap(entry => {
          const child = join(path, entry.name);
          return entry.isDirectory() ? walk(child).map(name => `${entry.name}/${name}`) : [entry.name];
        });
        call.packageFiles = walk(root);
        call.packageDocuments = Object.fromEntries(call.packageFiles
          .filter(path => DOCUMENTS.some(filename => path.endsWith(`/${filename}`)))
          .map(path => [path, readFileSync(join(root, path))]));
        const scriptsPath = args[args.indexOf("--scripts") + 1];
        call.preinstall = readFileSync(join(scriptsPath, "preinstall"), "utf8");
        call.postinstall = readFileSync(join(scriptsPath, "postinstall"), "utf8");
        const componentPlist = args[args.indexOf("--component-plist") + 1];
        call.componentPolicy = analyzedPolicies.get(componentPlist);
        for (const script of checkScriptSyntax ? ["preinstall", "postinstall"] : []) {
          const checked = spawnSync("/bin/sh", ["-n", join(scriptsPath, script)], { encoding: "utf8" });
          if (checked.status !== 0) throw new Error(`${script} syntax failed: ${checked.stderr}`);
        }
      } else {
        call.distribution = readFileSync(args[args.indexOf("--distribution") + 1], "utf8");
        const resourcesPath = args[args.indexOf("--resources") + 1];
        call.resources = Object.fromEntries(readdirSync(resourcesPath, { recursive: true }).filter(filename => fs.lstatSync(join(resourcesPath, filename)).isFile()).map(filename => [filename, readFileSync(join(resourcesPath, filename))]));
      }
      writeFileSync(args.at(-1), `${command} fixture`);
      return { status: 0, stdout: "" };
    }
    if (command === "/usr/sbin/pkgutil") {
      return { status: 0, stdout: packageSignature(signature) };
    }
    throw new Error(`unexpected command: ${command}`);
  };
}

test("parseArgs requires a matching Developer ID Installer identity and absolute inputs", () => {
  const parsed = parseArgs([
    "--payload", "/release/prepared payload",
    "--installerCli", "/release/mixrelay-installer",
    "--payloadChecker", "/release/mixrelay-macos-payload-check",
    "--teamId", TEAM,
    "--version", "1.2.3",
    "--signingIdentity", IDENTITY,
    "--output", "/release/packages",
  ]);
  assert.equal(parsed.signingIdentity, IDENTITY);
  assert.throws(() => parseArgs([
    "--payload", "/a", "--installerCli", "/b", "--payloadChecker", "/check", "--teamId", TEAM,
    "--version", "1.2", "--signingIdentity", `Developer ID Application: MixRelay (${TEAM})`, "--output", "/c",
  ]), /Developer ID Installer/);
  assert.throws(() => parseArgs([
    "--payload", "/a", "--installerCli", "/b", "--payloadChecker", "/check", "--teamId", TEAM,
    "--version", "1.2", "--signingIdentity", IDENTITY,
  ]), /Missing required option: --output/);
  assert.throws(() => parseArgs([
    "--payload", "/a", "--installerCli", "/b", "--teamId", TEAM,
    "--version", "1.2", "--signingIdentity", IDENTITY, "--output", "/c",
  ]), /Missing required option: --payloadChecker/);
  assert.throws(() => parseArgs([
    "--payload", "/a", "--installerCli", "/b", "--payloadChecker", "relative-checker",
    "--teamId", TEAM, "--version", "1.2", "--signingIdentity", IDENTITY, "--output", "/c",
  ]), /--payloadChecker must be an absolute/);
});

test("builds one installation/update package with an independently installed removal app", (t) => {
  const paths = fixture(t);
  const calls = [];
  const result = buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls),
  });
  assert.equal(result.status, "signed-artifacts-built");
  assert.deepEqual(readdirSync(paths.output).sort(), [
    "MixRelay-Installer-1.2.3.pkg",
    "build-result.json",
  ]);
  assert.match(result.stagingIds.installer, /^[a-f0-9]{32}$/);
  assert.deepEqual(Object.keys(result.stagingIds), ["installer"]);
  assert.equal(result.uninstaller, undefined);
  const payloadCheck = calls.find(call => call.command === paths.payloadChecker);
  assert.deepEqual(payloadCheck.args, [paths.payload, TEAM, "mixrelay-preflight", "staff", "/Users/mixrelay-preflight", paths.installerCli]);
  assert.equal(payloadCheck.environment.MIXRELAY_CODESIGN_IDENTITY, undefined);
  assert.equal(payloadCheck.environment.APPLE_SIGNING_IDENTITY, undefined);
  assert.ok(calls.indexOf(payloadCheck) < calls.findIndex(call => call.command === "/usr/bin/pkgbuild"));

  const componentBuilds = calls.filter(call => call.command === "/usr/bin/pkgbuild");
  const componentAnalyses = componentBuilds.filter(call => call.args[0] === "--analyze");
  const componentPackages = componentBuilds.filter(call => call.args[0] !== "--analyze");
  const productBuilds = calls.filter(call => call.command === "/usr/bin/productbuild");
  assert.equal(componentAnalyses.length, 1);
  assert.equal(componentPackages.length, 1);
  assert.equal(productBuilds.length, 1);
  assert.match(productBuilds[0].distribution, /<title>MixRelay<\/title>/);
  for (const call of [...componentPackages, ...productBuilds]) {
    assert.ok(call.args.includes("--sign"));
    assert.ok(call.args.includes(IDENTITY));
    assert.ok(call.args.includes("--timestamp"));
  }
  assert.ok(componentPackages[0].args.includes("--component-plist"));
  assert.ok(componentPackages[0].packageFiles.some(path => path.endsWith("payload/Applications/MixRelay/MixRelay.app/Contents/MacOS/main")));
  assert.ok(componentPackages[0].packageFiles.some(path => path.includes("Uninstall MixRelay.app")));
  assert.ok(!componentPackages[0].args.includes("--nopayload"));
  for (const filename of DOCUMENTS) {
    const source = readFileSync(join(paths.payload, "Applications/MixRelay/MixRelay.app/Contents/Resources", filename));
    const installerPath = componentPackages[0].packageFiles.find(path => path.endsWith(`payload/Applications/MixRelay/MixRelay.app/Contents/Resources/${filename}`));
    assert.ok(installerPath);
    assert.deepEqual(componentPackages[0].packageDocuments[installerPath], source);
  }
  for (const product of productBuilds) {
    assert.deepEqual(Object.keys(product.resources).sort(), [...LANGUAGES.flatMap(({language}) => [`${language}.lproj/INSTALLER-README.rtf`, `${language}.lproj/Localizable.strings`])].sort());
    assert.match(product.distribution, /<readme file="INSTALLER-README\.rtf" mime-type="text\/rtf"\/>/);
    const noticePath = componentPackages[0].packageFiles.find(path => path.endsWith("payload/Applications/MixRelay/MixRelay.app/Contents/Resources/LICENSE.txt"));
    assertCopyrightNotice(product.resources, componentPackages[0].packageDocuments[noticePath]);
    assertInstallerReadme(product.resources);
  }
  const policyEntries = [];
  const flatten = entries => {
    for (const entry of entries ?? []) {
      policyEntries.push(entry);
      flatten(entry.ChildBundles);
    }
  };
  flatten(componentPackages[0].componentPolicy);
  assert.ok(policyEntries.length >= 3, "the manager, HAL driver, and nested extension are all analyzed");
  assert.ok(policyEntries.some(entry => entry.RootRelativeBundlePath.endsWith("/Applications/MixRelay/MixRelay.app")));
  assert.ok(policyEntries.some(entry => entry.RootRelativeBundlePath.endsWith("/MixRelayHAL.driver")));
  assert.ok(policyEntries.some(entry => entry.RootRelativeBundlePath.endsWith("/MixRelayEditor.appex")));
  assert.ok(policyEntries.some(entry => entry.RootRelativeBundlePath.endsWith("/Uninstall MixRelay.app")));
  for (const entry of policyEntries) {
    assert.equal(entry.BundleIsRelocatable, false);
    assert.equal(entry.BundleIsVersionChecked, false);
    assert.equal(entry.BundleHasStrictIdentifier, true);
    assert.equal(entry.BundleOverwriteAction, "upgrade");
  }
  const installPostinstall = componentPackages[0].postinstall;
  const installPreinstall = componentPackages[0].preinstall;
  assert.match(installPostinstall, /"\$stage\/payload\/Library\/Application Support\/MixRelay\/bin\/mixrelay-driver-service"/);
  assert.match(installPostinstall, /stage_identity=.*stat -f '%d:%i'/);
  assert.match(installPostinstall, /current_identity=.*stat -f '%d:%i'/);
  assert.match(installPostinstall, /\/bin\/rm -rf "\$stage"/);
  assert.match(installPostinstall, /check_shared_application_support_dir "\$3\/Library\/Application Support"/);
  const installCommand = installPostinstall.replace(/[ \t]*\\\n[ \t]*/g, " ").split("\n").find(line => line.startsWith('"$cli" install '));
  assert.equal(installCommand, `"$cli" install ${TEAM} "$stage/payload/Applications/MixRelay/MixRelay.app" "$stage/payload/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver" "$stage/payload/Library/Application Support/MixRelay/bin/mixrelay-driver-service" "$stage/payload/templates/io.mixrelay.audio.driver.plist.in" "$stage/payload/Applications/MixRelay/Uninstall MixRelay.app" "$stage/payload/templates/io.mixrelay.driver.removal.plist" "$stage/payload/Library/Application Support/MixRelay/bin/mixrelay-service" "$stage/payload/templates/io.mixrelay.audio.engine.plist.in" --reload-coreaudio`);
  assert.doesNotMatch(installPostinstall, /--delete-user-data/);
  assert.match(installPreinstall, /unexpected file in interrupted package inputs/);
  assert.match(installPreinstall, /private package staging inode changed; left it untouched/);
  assert.match(installPreinstall, /stat -f '%u:%g:%Lp'/);
  assert.match(installPreinstall, /check_shared_application_support_dir "\$3\/Library\/Application Support"/);
  for (const script of [installPostinstall, installPreinstall]) {
    assert.match(script, /root:admin:755\|root:wheel:755/);
    assert.doesNotMatch(script, /check_dir "\$3\/Library\/Application Support"/);
  }
  const distributions = productBuilds.map(call => call.distribution);
  assert.equal(distributions.length, 1);
  assert.match(distributions[0], /customize="never"/);
  assert.match(distributions[0], /allow-external-scripts="false"/);
  assert.doesNotMatch(distributions[0], /system\.run\(/);
  assert.match(distributions[0], /<title>MixRelay<\/title>/);
  assert.doesNotMatch(distributions[0], /<license\b/);
  assert.match(distributions[0], /<pkg-ref id="io\.mixrelay\.installer\.component" version="1\.2\.3">MixRelay-Installer-1\.2\.3\.component\.pkg<\/pkg-ref>/);
  const closeReference = distributions[0].match(/<pkg-ref id="io\.mixrelay\.installer\.component">\s*<must-close>([\s\S]*?)<\/must-close>\s*<\/pkg-ref>/);
  assert.ok(closeReference, "native close requirements use a separate reference to the same component package");
  assert.deepEqual([...closeReference[1].matchAll(/<app id="([^"]+)"\/>/g)].map(match => match[1]), [
    "io.mixrelay.driver.uninstaller",
  ]);
  assert.equal([...distributions[0].matchAll(/<must-close>/g)].length, 1);
  assert.doesNotMatch(distributions[0], /deleteUserData/);
  assert.doesNotMatch(distributions[0], /<choice id="remove|delete-user-data|remover\.(?:keep|delete)/);
  for (const text of distributions) {
    assert.match(text, /enable_anywhere="false"/);
    assert.match(text, /enable_localSystem="true"/);
    assert.doesNotMatch(text, /RequireRestart|onConclusion=/);
  }
});

const SUBMISSION = "12345678-1234-1234-1234-123456789abc";
const API_KEY = { apiKeyPath: "/private/fixture/AuthKey.p8", keyId: "ZX98Y7W6V5", issuer: "87654321-4321-4321-4321-cba987654321" };
const savedBuild = paths => JSON.parse(readFileSync(join(paths.output, "build-result.json"), "utf8"));
function notarizedBuild(paths, calls, xcrun, options = {}) {
  const packaging = fakeRunner(calls, IDENTITY, () => {}, undefined, false);
  return buildPackages({
    ...paths, teamId: TEAM, version: "1.2.3", signingIdentity: IDENTITY,
    skipNotarization: false, notarization: API_KEY, ...options,
    run: (program, args, settings) => {
      if (program === "/usr/bin/xcrun" && ["notarytool", "stapler"].includes(args[0])) {
        return xcrun(args, settings);
      }
      return packaging(program, args, settings);
    },
  });
}

const KEY_ARGS = ["--key", API_KEY.apiKeyPath, "--key-id", API_KEY.keyId, "--issuer", API_KEY.issuer];
for (const [name, credentials, authentication] of [
  ["API key references", API_KEY, KEY_ARGS],
]) {
  test(`notarization submits the final signed PKG with ${name}, then staples and validates it`, t => {
    const paths = fixture(t);
    const packaging = [];
    const invocations = [];
    const pending = [];
    const installer = join(paths.output, "MixRelay-Installer-1.2.3.pkg");
    const result = notarizedBuild(paths, packaging, (args, settings) => {
      invocations.push(args);
      pending.push(savedBuild(paths).notarization.status);
      assert.deepEqual(settings.stdio, ["ignore", "pipe", "pipe"], "credential diagnostics stay captured");
      assert.equal(settings.encoding, "utf8");
      assert.ok(packaging.some(call => call.command === "/usr/sbin/pkgutil"), "signature gate precedes submission");
      assert.ok(fs.existsSync(installer), "submission and stapling use the published final PKG");
      if (args[1] === "submit") return { status: 0, stdout: JSON.stringify({ id: SUBMISSION, status: "Accepted" }) };
      if (args[1] === "log") return { status: 0, stdout: JSON.stringify({ status: "Accepted", issues: [{ severity: "warning", message: "fixture warning" }] }) };
      if (args[1] === "staple") fs.appendFileSync(installer, " stapled fixture");
      if (args[1] === "validate") assert.match(readFileSync(installer, "utf8"), /stapled fixture$/);
      return { status: 0, stdout: "success" };
    }, { notarization: credentials });
    assert.deepEqual(invocations, [
      ["notarytool", "submit", installer, "--wait", "--timeout", "2h", "--output-format", "json", ...authentication],
      ["notarytool", "log", SUBMISSION, ...authentication],
      ["stapler", "staple", installer], ["stapler", "validate", installer],
    ]);
    assert.deepEqual(pending, ["submitting", "submitting", "stapling", "validating"]);
    assert.equal(result.status, "notarized-artifacts-built");
    assert.deepEqual(result.notarization, { status: "accepted", notarized: true, stapled: true, validated: true, submissionId: SUBMISSION, appleStatus: "Accepted", logStatus: "fetched", logPath: join(paths.output, "notarization-log.json") });
    assert.equal(JSON.parse(readFileSync(result.notarization.logPath, "utf8")).issues[0].message, "fixture warning", "accepted submissions keep Apple's warnings");
    assert.deepEqual(savedBuild(paths), result);
    for (const value of Object.values(credentials)) assert.ok(!JSON.stringify(result).includes(value), "results omit credential references");
    assert.equal(readdirSync(paths.output).some(name => name.startsWith(".mixrelay-pkg-build-")), false);
  });
}

test("notarization rejection fetches a diagnostic log and fails without stapling", t => {
  const paths = fixture(t);
  const invocations = [];
  let failure;
  try {
    notarizedBuild(paths, [], args => {
      invocations.push(args);
      if (args[1] === "submit") return { status: 65, stdout: JSON.stringify({ id: SUBMISSION, status: "Invalid" }), stderr: API_KEY.keyId };
      assert.deepEqual(args, ["notarytool", "log", SUBMISSION, ...KEY_ARGS]);
      return { status: 0, stdout: JSON.stringify({ issues: [{ message: "Unsigned nested binary" }], diagnostic: API_KEY.keyId }) };
    });
    assert.fail("a rejected package must fail the release");
  } catch (error) { failure = error; }
  assert.match(failure.message, /notarization failed during submit/);
  assert.ok(!failure.message.includes(API_KEY.keyId));
  const result = savedBuild(paths);
  assert.deepEqual(failure.result, result);
  assert.equal(result.status, "notarization-failed");
  assert.deepEqual(result.notarization, {
    status: "failed", notarized: false, stapled: false, validated: false,
    submissionId: SUBMISSION, appleStatus: "Invalid", failedStep: "submit", logStatus: "fetched", logPath: join(paths.output, "notarization-log.json"),
  });
  const log = JSON.parse(readFileSync(result.notarization.logPath, "utf8"));
  assert.equal(log.issues[0].message, "Unsigned nested binary");
  assert.equal(log.diagnostic, "[redacted]");
  assert.equal(invocations.length, 2);
  assert.equal(fs.existsSync(result.installer), true, "retain the signed PKG for diagnosis");
  assert.equal(readdirSync(paths.output).some(name => name.startsWith(".mixrelay-pkg-build-")), false);
});

for (const step of ["submit", "staple", "validate"]) {
  test(`notarization interrupted during ${step} stops without further xcrun work`, t => {
    const paths = fixture(t);
    const invocations = [];
    assert.throws(() => notarizedBuild(paths, [], args => {
      invocations.push(args[1]);
      if (args[1] === step) return { status: null, signal: "SIGTERM" };
      return { status: 0, stdout: JSON.stringify({ id: SUBMISSION, status: "Accepted" }) };
    }), /notarization interrupted/);
    const order = ["submit", "log", "staple", "validate"];
    assert.deepEqual(invocations, order.slice(0, order.indexOf(step) + 1));
    const result = savedBuild(paths);
    assert.equal(result.status, "notarization-failed");
    assert.equal(result.notarization.status, "interrupted");
    assert.equal(result.notarization.failedStep, step);
    assert.equal(result.notarization.validated, false);
    assert.equal(result.notarization.notarized, step !== "submit");
    assert.equal(result.notarization.stapled, step === "validate");
  });
}

for (const [name, reply, expectedLog] of [
  ["zero exit with rejection", { status: 0, stdout: JSON.stringify({ id: SUBMISSION, status: "Invalid" }) }, true],
  ["nonzero exit with acceptance", { status: 1, stdout: JSON.stringify({ id: SUBMISSION, status: "Accepted" }) }, true],
  ["pending verdict", { status: 0, stdout: JSON.stringify({ id: SUBMISSION, status: "In Progress" }) }, true],
  ["malformed JSON", { status: 0, stdout: "not JSON" }, false],
  ["missing submission id", { status: 0, stdout: JSON.stringify({ status: "Accepted" }) }, false],
  ["spawn failure", { status: null, error: new Error("fixture sensitive diagnostic") }, false],
]) {
  test(`notarization fails closed for ${name}, even if log retrieval fails`, t => {
    const paths = fixture(t);
    const invocations = [];
    assert.throws(() => notarizedBuild(paths, [], args => {
      invocations.push(args[1]);
      return args[1] === "submit" ? reply : { status: 1, stderr: "fixture sensitive diagnostic" };
    }), error => /notarization failed/.test(error.message) && !error.message.includes("sensitive diagnostic"));
    assert.deepEqual(invocations, expectedLog ? ["submit", "log"] : ["submit"]);
    const result = savedBuild(paths);
    assert.equal(result.notarization.notarized, false);
    assert.equal(result.notarization.logStatus, expectedLog ? "failed" : undefined);
    assert.equal(fs.existsSync(join(paths.output, "notarization-log.json")), false);
  });
}

for (const step of ["staple", "validate"]) {
  test(`notarization cannot qualify a PKG when ${step} fails`, t => {
    const paths = fixture(t);
    const invocations = [];
    assert.throws(() => notarizedBuild(paths, [], args => {
      invocations.push(args[1]);
      if (args[1] === step) return { status: 1 };
      return { status: 0, stdout: JSON.stringify({ id: SUBMISSION, status: "Accepted" }) };
    }), new RegExp(`notarization failed during ${step}`));
    // The log was already fetched after acceptance; a later failure does not fetch it again.
    assert.deepEqual(invocations, step === "staple" ? ["submit", "log", "staple"] : ["submit", "log", "staple", "validate"]);
    const outcome = savedBuild(paths).notarization;
    assert.equal(outcome.notarized, true);
    assert.equal(outcome.stapled, step === "validate");
    assert.equal(outcome.validated, false);
    assert.equal(outcome.failedStep, step);
  });
}

test("notarization credentials are required before any package tool or output is touched", t => {
  const paths = fixture(t);
  const calls = [];
  for (const notarization of [undefined, {}, { keychainProfile: "fixture-notary-profile" }, { apiKeyPath: "/fixture/key.p8" }, { ...API_KEY, apiKeyPath: "relative.p8" }, { keychainProfile: "fixture-notary-profile", ...API_KEY }, { ...API_KEY, password: "inline-secret" }]) {
    assert.throws(() => notarizedBuild(paths, calls, () => assert.fail("xcrun must not run"), { notarization }), /Signing config requires notarization/);
  }
  assert.deepEqual(calls, []);
  assert.equal(fs.existsSync(paths.output), false);
});

test("notarization skip is explicit in arguments and marks the local PKG not notarized", t => {
  const paths = fixture(t);
  const calls = [];
  const result = notarizedBuild(paths, calls, () => assert.fail("skip must not contact Apple"), { skipNotarization: true, notarization: undefined });
  assert.deepEqual(result.notarization, { status: "skipped", notarized: false, stapled: false, validated: false });
  assert.equal(result.status, "signed-artifacts-built");
  assert.deepEqual(savedBuild(paths), result);
  const required = ["--payload", "/a", "--installerCli", "/b", "--payloadChecker", "/check", "--teamId", TEAM, "--version", "1.2", "--signingIdentity", IDENTITY, "--output", "/c"];
  assert.equal(parseArgs(required).skipNotarization, false);
  assert.equal(parseArgs([...required, "--skip-notarization"]).skipNotarization, true);
});

test("RTF installation guidance preserves Unicode and escaped text", () => {
  const notice = "MixRelay\r\nInstallation Guide\r\n\r\nBefore installation\r\n"
    + String.raw`Keep \path{guide} and literal \u123? intact.` + "\r\n한국어 — café 😀\r\nwrapped body continues.\r\n";
  const rtf = installerRtf(notice);
  assert.equal(presentationWords(rtfText(Buffer.from(rtf))), presentationWords(notice));
  assert.ok(rtf.includes(String.raw`\\path\{guide\}`));
  assert.ok(rtf.includes(String.raw`\\u123?`));
  assert.ok(rtf.includes(String.raw`\u-10179?\u-8704?`));
  assert.doesNotMatch(rtf, /[^\x00-\x7f]/);
});

test("generated package scripts require three arguments and accept trailing Installer context", (t) => {
  const paths = fixture(t);
  const calls = [];
  buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls),
  });
  const components = calls.filter(call => call.command === "/usr/bin/pkgbuild" && call.args.includes("--root") && call.args[0] !== "--analyze");
  assert.equal(components.length, 1, "only the installation component has package scripts");
  const installerArgs = ["fixture.pkg", "/", join(paths.root, "off-system-volume"), "Installer context", "more context"];
  for (const component of components) {
    const identifier = component.args[component.args.indexOf("--identifier") + 1];
    for (const phase of ["preinstall", "postinstall"]) {
      for (const count of [0, 1, 2, 3, 4, 5]) {
        // An off-system target stops at the next gate, before any system inspection or installer CLI.
        const result = spawnSync("/bin/sh", ["-c", component[phase], phase, ...installerArgs.slice(0, count)], {
          encoding: "utf8",
          timeout: 5_000,
        });
        const context = `${identifier} ${phase}, ${count} arguments`;
        assert.equal(result.error, undefined, context);
        assert.equal(result.status, 1, context);
        assert.equal(result.stdout, "", context);
        if (count < 3) {
          assert.match(result.stderr, new RegExp(`^MixRelay (?:package|installer): expected at least 3 Installer script arguments; received ${count}\\n$`), context);
        } else {
          assert.match(result.stderr, /^MixRelay (?:package|installer): the signed product may only install on the system volume\n$/, context);
        }
      }
    }
  }
});

test("preinstall retries only trusted exact package inputs after interrupted extraction", async (t) => {
  for (const scenario of ["complete extraction", "partial extraction", "stage symlink", "input symlink", "untrusted parent", "untrusted stage", "untrusted input", "unknown recovery directory", "changed inode"]) {
    await t.test(scenario, (t) => {
      const paths = fixture(t);
      const volume = join(paths.root, "simulated-volume");
      const calls = [];
      const result = buildPackages({
        ...paths, teamId: TEAM, version: "1.2.3", signingIdentity: IDENTITY,
        run: fakeRunner(calls, IDENTITY, packageRoot => fs.cpSync(packageRoot, volume, { recursive: true })),
      });
      const component = calls.find(call => call.command === "/usr/bin/pkgbuild" && call.args[0] !== "--analyze");
      const stage = join(volume, "Library/Application Support/MixRelay/.installer-inputs", result.stagingIds.installer);
      const cli = join(stage, "bin/mixrelay-macos-installer");
      const otherStage = join(volume, "Library/Application Support/MixRelay/.installer-inputs/other-package");
      const recovery = join(volume, "Library/Application Support/MixRelay/.native-recovery");
      for (const directory of [otherStage, recovery]) {
        mkdirSync(directory);
        writeFileSync(join(directory, "keep"), "unrelated recovery data");
      }
      if (scenario === "partial extraction") rmSync(join(stage, "payload/Applications"), { recursive: true });
      if (scenario === "stage symlink") {
        rmSync(stage, { recursive: true });
        symlinkSync(recovery, stage);
      }
      if (scenario === "input symlink") {
        rmSync(cli);
        symlinkSync(join(recovery, "keep"), cli);
      }
      if (scenario === "unknown recovery directory") mkdirSync(join(stage, ".unknown-recovery"));
      const commands = join(paths.root, "preinstall-commands");
      mkdirSync(commands);
      writeFileSync(join(commands, "id"), "#!/bin/sh\nprintf '0\\n'\n", { mode: 0o755 });
      // Native stat keeps descriptor/inode behavior real without launching a
      // JavaScript runtime once per fixture file; only ownership is simulated.
      const statArg = process.platform === "darwin" ? "-f" : "-c";
      const modeFormat = process.platform === "darwin" ? "%Lp" : "%a";
      writeFileSync(join(commands, "stat"), `#!/bin/sh
format=$2
fixture_path=$3
case "$format" in
  '%u:%g:%Lp')
    mode=$(/usr/bin/stat ${statArg} '${modeFormat}' "$fixture_path") || exit 1
    if [ "$TEST_UNTRUSTED_PATH" = "$fixture_path" ]; then printf '501:20:%s' "$mode"; else printf '0:0:%s' "$mode"; fi ;;
  '%Su:%Sg:%Lp') mode=$(/usr/bin/stat ${statArg} '${modeFormat}' "$fixture_path") || exit 1; printf 'root:admin:%s' "$mode" ;;
  '%u:%g') if [ "$TEST_UNTRUSTED_PATH" = "$fixture_path" ]; then printf '501:20'; else printf '0:0'; fi ;;
  '%d:%i')
    value=$(/usr/bin/stat ${statArg} '%d:%i' "$fixture_path") || exit 1
    count=0
    if [ -f "$TEST_STAT_COUNTER" ]; then read -r count < "$TEST_STAT_COUNTER"; fi
    printf '%s\\n' "$((count + 1))" > "$TEST_STAT_COUNTER"
    if [ "$TEST_CHANGED_INODE" = 1 ] && [ "$count" -gt 0 ]; then
      printf '%s:%s' "\${value%:*}" "$((\${value#*:} + 1))"
    else printf '%s' "$value"; fi ;;
  *) exit 2 ;;
esac
`, { mode: 0o755 });
      const script = component.preinstall
        .replaceAll("$3/Library", "$TEST_VOLUME/Library")
        .replaceAll("/usr/bin/id", '"$TEST_COMMANDS/id"')
        .replaceAll("/usr/bin/stat", '"$TEST_COMMANDS/stat"');
      const execution = spawnSync("/bin/sh", ["-c", script, "preinstall", "fixture.pkg", "/", "/"], {
        encoding: "utf8", timeout: 60_000,
        env: {
          ...process.env, TEST_VOLUME: volume, TEST_COMMANDS: commands,
          TEST_STAT_COUNTER: join(commands, "stage-stat-count"),
          TEST_UNTRUSTED_PATH: scenario === "untrusted stage" ? stage : scenario === "untrusted input" ? cli : scenario === "untrusted parent" ? dirname(stage) : "",
          TEST_CHANGED_INODE: scenario === "changed inode" ? "1" : "0",
        },
      });
      assert.equal(execution.error, undefined);
      const retryAllowed = scenario === "complete extraction" || scenario === "partial extraction";
      assert.equal(execution.status, retryAllowed ? 0 : 1, execution.stderr);
      if (retryAllowed) assert.equal(fs.existsSync(stage), false, "only interrupted extracted inputs are cleared for fresh extraction");
      else {
        assert.ok(fs.existsSync(stage) || fs.lstatSync(stage).isSymbolicLink(), "unsafe inputs stay untouched");
        assert.match(execution.stderr, /unsafe package parent|symlink|not root:wheel|unexpected file|inode changed/);
      }
      for (const directory of [otherStage, recovery]) {
        assert.equal(readFileSync(join(directory, "keep"), "utf8"), "unrelated recovery data");
      }
    });
  }
});

test("rejects an obsolete separate removal app before package creation", t => {
  const paths = fixture(t);
  mkdirSync(join(paths.payload, "Applications/MixRelay/MixRelay.app/Contents/Helpers/Uninstall MixRelay.app"), { recursive: true });
  const calls = [];
  assert.throws(() => buildPackages({ ...paths, teamId: TEAM, version: "1.2.3", signingIdentity: IDENTITY, run: fakeRunner(calls) }), /must not contain a separate uninstaller/);
  assert.equal(calls.length, 0);
  assert.equal(fs.existsSync(paths.output), false);
});

test("refuses unsigned or wrongly signed products and leaves no partial outputs", (t) => {
  const paths = fixture(t);
  assert.throws(() => buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: `Developer ID Application: MixRelay (${TEAM})`,
    run: fakeRunner([]),
  }), /Developer ID Installer/);
  assert.equal(fs.existsSync(paths.output), false);

  const calls = [];
  assert.throws(() => buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls, "Developer ID Installer: Different Publisher (ZZ99YY88XX)"),
  }), /signature is not trusted with the requested/);
  assert.deepEqual(fs.existsSync(paths.output) ? readdirSync(paths.output) : [], []);
});

test("pkgutil accepts only the two normal trusted statuses with the exact first Installer leaf and timestamp", async (t) => {
  for (const status of [
    "signed by a certificate trusted by macOS",
    "signed by a developer certificate issued by Apple for distribution",
  ]) {
    await t.test(status, (t) => {
      const paths = fixture(t);
      const base = fakeRunner([]);
      const result = buildPackages({
        ...paths, teamId: TEAM, version: "1.2.3", signingIdentity: IDENTITY,
        run: (command, args, options) => command === "/usr/sbin/pkgutil"
          ? { status: 0, stdout: packageSignature(IDENTITY, status) }
          : base(command, args, options),
      });
      assert.equal(result.status, "signed-artifacts-built");
      assert.ok(fs.existsSync(result.installer));
      assert.equal(result.uninstaller, undefined);
    });
  }
});

test("pkgutil rejects untrusted statuses, wrong or substring leaves, missing timestamps and nonzero exit", async (t) => {
  const trusted = packageSignature();
  const cases = [
    ...[
      "signed by untrusted certificate", "revoked signature", "invalid signature", "no signature",
      "signed by a certificate that has since expired", "signed by a certificate trusted for current user",
      "signed by a certificate trusted on this system", "signed by a developer certificate issued by Apple (Development)",
      "signed by a certificate trusted by macOS but untrusted", "signed by a developer certificate issued by Apple for distribution (Development)",
    ].map(status => [status, { status: 0, stdout: packageSignature(IDENTITY, status) }]),
    ["different first leaf, requested identity later", { status: 0, stdout: packageSignature("Developer ID Installer: Other Publisher (ZZ99YY88XX)").replace("2. Developer ID Certification Authority", `2. ${IDENTITY}`) }],
    ["requested identity only a leaf substring", { status: 0, stdout: packageSignature(`${IDENTITY} Extra`) }],
    ["identity outside the certificate chain", { status: 0, stdout: trusted.replace("   Certificate Chain:\n", "") }],
    ["missing trusted timestamp", { status: 0, stdout: trusted.replace(/^.*Signed with a trusted timestamp.*\n/m, "") }],
    ["empty trusted timestamp", { status: 0, stdout: trusted.replace("2026-10-05 21:06:51 +0000", "") }],
    ["nonzero exit despite plausible stdout", { status: 1, stdout: trusted, stderr: "signature validation failed" }],
  ];
  for (const [name, signature] of cases) {
    await t.test(name, (t) => {
      const paths = fixture(t);
      const base = fakeRunner([]);
      assert.throws(() => buildPackages({
        ...paths, teamId: TEAM, version: "1.2.3", signingIdentity: IDENTITY,
        run: (command, args, options) => command === "/usr/sbin/pkgutil"
          ? signature : base(command, args, options),
      }), error => {
        assert.match(error.message, /signature is not trusted with the requested|verify signed (?:install|removal) package failed/);
        assert.ok(error.message.includes(signature.stdout.trim()), "failure must preserve actual pkgutil output");
        if (signature.stderr) assert.ok(error.message.includes(signature.stderr));
        return true;
      });
      assert.deepEqual(fs.existsSync(paths.output) ? readdirSync(paths.output) : [], []);
    });
  }
});

test("payload signature/team failure is rejected before package staging or signing", (t) => {
  const paths = fixture(t);
  const calls = [];
  assert.throws(() => buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls, IDENTITY, () => {}, { status: 1, stderr: "Developer ID signature rejected" }),
  }), /Developer ID Application payload preflight failed: Developer ID signature rejected/);
  assert.ok(calls.some(call => call.command === paths.payloadChecker));
  assert.equal(calls.some(call => call.command === "/usr/bin/pkgbuild" || call.command === "/usr/bin/productbuild"), false);
  assert.equal(fs.existsSync(paths.output), false);
});

test("payload checker must be an explicit regular executable before any native package command", (t) => {
  const paths = fixture(t);
  chmodSync(paths.payloadChecker, 0o644);
  const calls = [];
  assert.throws(() => buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls),
  }), /payload checker must be a non-empty executable/);
  assert.equal(calls.some(call => call.command === paths.payloadChecker || call.command === "/usr/bin/pkgbuild"), false);
  assert.equal(fs.existsSync(paths.output), false);
});

test("payload checker path must identify the release verifier binary", (t) => {
  const paths = fixture(t);
  const wrongName = join(paths.root, "unrelated-checker");
  writeFileSync(wrongName, "not the release verifier");
  chmodSync(wrongName, 0o755);
  const calls = [];
  assert.throws(() => buildPackages({
    ...paths,
    payloadChecker: wrongName,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls),
  }), /must name the mixrelay-macos-payload-check binary/);
  assert.equal(calls.some(call => call.command === wrongName || call.command === "/usr/bin/pkgbuild"), false);
  assert.equal(fs.existsSync(paths.output), false);
});

test("normalizes package staging directories despite a restrictive umask", (t) => {
  const paths = fixture(t);
  const restrictiveFs = {
    ...fs,
    mkdirSync(path, options = {}) {
      const mode = options.mode ?? 0o777;
      return fs.mkdirSync(path, { ...options, mode: mode & ~0o077 });
    },
  };
  const calls = [];
  const inspectPackageRoot = packageRoot => {
    const directories = [];
    const visit = directory => {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        if (!entry.isDirectory()) continue;
        const child = join(directory, entry.name);
        directories.push(child);
        visit(child);
      }
    };
    visit(packageRoot);
    const bundleRoots = directories.filter(path => [".app", ".driver", ".appex", ".framework", ".bundle", ".xpc"]
      .some(extension => path.endsWith(extension)));
    for (const directory of directories) {
      if (bundleRoots.some(bundle => directory === bundle || directory.startsWith(`${bundle}/`))) continue;
      assert.equal(fs.statSync(directory).mode & 0o777, 0o755, directory);
    }
    for (const bundle of bundleRoots) {
      assert.equal(fs.statSync(bundle).mode & 0o777, 0o755, bundle);
    }
  };
  const result = buildPackages({
    ...paths,
    fs: restrictiveFs,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls, IDENTITY, inspectPackageRoot),
  });
  assert.equal(result.status, "signed-artifacts-built");
});

test("rejects payload bundle roots that would fail postinstall ownership checks", (t) => {
  const paths = fixture(t);
  chmodSync(join(paths.payload, "Applications/MixRelay/MixRelay.app"), 0o700);
  const calls = [];
  assert.throws(() => buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls),
  }), /manager payload bundle root must have mode 0755/);
  assert.equal(calls.length, 0);
});

for (const filename of DOCUMENTS) {
  for (const kind of ["missing", "empty"]) {
    test(`rejects ${kind} ${filename} before package commands`, (t) => {
      const paths = fixture(t);
      const document = join(paths.payload, "Applications/MixRelay/MixRelay.app/Contents/Resources", filename);
      if (kind === "missing") rmSync(document);
      else writeFileSync(document, "");
      const calls = [];
      assert.throws(() => buildPackages({
        ...paths, teamId: TEAM, version: "1.2.3", signingIdentity: IDENTITY, run: fakeRunner(calls),
      }), /Missing release document path component|release document must be non-empty/);
      assert.equal(calls.length, 0);
      assert.equal(fs.existsSync(paths.output), false);
    });
  }
}

test("does not overwrite a package path that appears during publication", (t) => {
  const paths = fixture(t);
  const installer = join(paths.output, "MixRelay-Installer-1.2.3.pkg");
  const safeFs = {
    ...fs,
    linkSync(source, path) {
      if (path === installer) {
        writeFileSync(path, "concurrent user file");
        const error = new Error("destination exists");
        error.code = "EEXIST";
        throw error;
      }
      return fs.linkSync(source, path);
    },
  };
  assert.throws(() => buildPackages({
    ...paths,
    fs: safeFs,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner([]),
  }), /destination exists/);
  assert.deepEqual(readdirSync(paths.output), ["MixRelay-Installer-1.2.3.pkg"]);
  assert.equal(readFileSync(installer, "utf8"), "concurrent user file");
});

test("rejects symlinked payloads, unprepared manifests and nonempty output before package commands", (t) => {
  const paths = fixture(t);
  const alias = join(paths.root, "payload-link");
  symlinkSync(paths.payload, alias);
  const calls = [];
  assert.throws(() => buildPackages({
    ...paths,
    payload: alias,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls),
  }), /must not contain symlinks/);
  assert.equal(calls.length, 0);

  writeFileSync(join(paths.payload, "payload-manifest.json"), "{}");
  assert.throws(() => buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls),
  }), /fixed MixRelay install contract/);
  assert.equal(calls.length, 0);

  writeFileSync(join(paths.payload, "payload-manifest.json"), JSON.stringify({
    format: 1,
    status: "payload-prepared-not-package",
    installContract: {
      productDirectory: "/Applications/MixRelay",
      manager: "/Applications/MixRelay/MixRelay.app",
      uninstaller: "/Applications/MixRelay/Uninstall MixRelay.app",
      removalLaunchd: "/Library/LaunchDaemons/io.mixrelay.driver.removal.plist",
      engine: "/Library/Application Support/MixRelay/bin/mixrelay-service",
      engineLaunchd: "/Library/LaunchDaemons/io.mixrelay.audio.engine.plist",
      hal: "/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver",
      driver: "/Library/Application Support/MixRelay/bin/mixrelay-driver-service",
      launchd: "/Library/LaunchDaemons/io.mixrelay.audio.driver.plist",
    },
  }));
  mkdirSync(paths.output);
  writeFileSync(join(paths.output, "keep"), "keep");
  assert.throws(() => buildPackages({
    ...paths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(calls),
  }), /must be empty/);
  assert.equal(readFileSync(join(paths.output, "keep"), "utf8"), "keep");
  assert.equal(calls.some(call => call.command === "/usr/bin/pkgbuild" || call.command === "/usr/bin/productbuild"), false);
});

test("fixed component policy rejects unpinned or out-of-scope analyzed bundles", () => {
  const packageRoot = "/fixture/mixrelay-package-root";
  const manager = join(packageRoot, "stage/payload/Applications/MixRelay/MixRelay.app");
  const hal = join(packageRoot, "stage/payload/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver");
  const appRelative = relative(packageRoot, manager);
  const halRelative = relative(packageRoot, hal);
  const analyzed = [
    { RootRelativeBundlePath: appRelative, ChildBundles: [{ RootRelativeBundlePath: `${appRelative}/Contents/PlugIns/Editor.appex` }] },
    { RootRelativeBundlePath: halRelative },
  ];
  const result = fixedComponentBundlePolicy(analyzed, packageRoot, [manager, hal]);
  assert.equal(result.bundlePaths.length, 3);
  assert.ok(result.bundlePaths.includes(`${appRelative}/Contents/PlugIns/Editor.appex`));
  assert.throws(() => fixedComponentBundlePolicy([
    ...analyzed,
    { RootRelativeBundlePath: "Applications/Other.app" },
  ], packageRoot, [manager, hal]), /outside the fixed manager\/HAL payloads/);
  assert.throws(() => fixedComponentBundlePolicy([
    { RootRelativeBundlePath: "../Applications/Moved.app" },
    analyzed[1],
  ], packageRoot, [manager, hal]), /unsafe bundle staging path/);
});

test("native pkgbuild expands fixed-path bundles without relocation or version-search clauses", { skip: process.platform !== "darwin" }, (t) => {
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const target = join(repoRoot, "target");
  mkdirSync(target, { recursive: true });
  const root = mkdtempSync(join(target, "macos-component-policy-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const packageRoot = join(root, "package-root");
  const manager = join(packageRoot, "Library/Application Support/MixRelay/.installer-inputs/fixture/payload/Applications/MixRelay/MixRelay.app");
  const hal = join(packageRoot, "Library/Application Support/MixRelay/.installer-inputs/fixture/payload/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver");
  const nested = join(manager, "Contents/PlugIns/MixRelayEditor.appex");
  for (const [bundle, identifier, type] of [
    [manager, "io.mixrelay.desktop", "APPL"],
    [hal, "io.mixrelay.audio.MixRelayHAL", "BNDL"],
    [nested, "io.mixrelay.desktop.editor", "XPC!"],
  ]) {
    mkdirSync(join(bundle, "Contents/MacOS"), { recursive: true });
    writeFileSync(join(bundle, "Contents/MacOS/main"), "fixture binary");
    writeFileSync(join(bundle, "Contents/Info.plist"), `<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${identifier}</string><key>CFBundleName</key><string>MixRelay</string><key>CFBundlePackageType</key><string>${type}</string><key>CFBundleVersion</key><string>1.2.3</string></dict></plist>`);
  }
  const resourcesPath = join(root, "resources");
  mkdirSync(resourcesPath);
  mkdirSync(join(manager, "Contents/Resources"));
  for (const [filename, contents] of [
    ["LICENSE.txt", SOURCE_DOCUMENTS["LICENSE.txt"]],
  ]) {
    writeFileSync(join(manager, "Contents/Resources", filename), contents);
  }
  const generatedPaths = fixture(t);
  const generatedCalls = [];
  const generatedResult = buildPackages({
    ...generatedPaths,
    teamId: TEAM,
    version: "1.2.3",
    signingIdentity: IDENTITY,
    run: fakeRunner(generatedCalls),
  });
  const generatedResources = generatedCalls.find(call => call.command === "/usr/bin/productbuild").resources;
  // productbuild includes Distribution-referenced resources; standalone documents stay in the component payload.
  const expectedProductResources = Object.fromEntries(Object.entries(generatedResources).filter(([filename]) => filename.endsWith(".rtf") || filename.endsWith(".strings")));
  for (const [filename, contents] of Object.entries(generatedResources)) {
    mkdirSync(dirname(join(resourcesPath, filename)), { recursive: true });
    writeFileSync(join(resourcesPath, filename), contents);
  }
  const analyzedPath = join(root, "analyzed.plist");
  const componentJsonPath = join(root, "components.json");
  const fixedPlistPath = join(root, "components.plist");
  const packagePath = join(root, "fixture.component.pkg");
  const expandedPath = join(root, "expanded");
  const runNative = (command, args) => {
    const result = spawnSync(command, args, { encoding: "utf8", env: { ...process.env, LC_ALL: "C" }, maxBuffer: 4 * 1024 * 1024, timeout: 30_000 });
    assert.equal(result.error, undefined, `${command}: ${result.error?.message ?? ""}`);
    assert.equal(result.status, 0, `${command} ${args.join(" ")}: ${result.stderr}`);
    return result.stdout;
  };

  runNative("/usr/bin/pkgbuild", ["--analyze", "--root", packageRoot, analyzedPath]);
  const analyzed = JSON.parse(runNative("/usr/bin/plutil", ["-convert", "json", "-o", "-", analyzedPath]));
  const fixed = fixedComponentBundlePolicy(analyzed, packageRoot, [manager, hal]);
  assert.ok(fixed.bundlePaths.includes(relative(packageRoot, nested)));
  writeFileSync(componentJsonPath, JSON.stringify(fixed.componentPlist));
  runNative("/usr/bin/plutil", ["-convert", "xml1", "-o", fixedPlistPath, componentJsonPath]);
  runNative("/usr/bin/pkgbuild", [
    "--root", packageRoot,
    "--component-plist", fixedPlistPath,
    "--identifier", "io.mixrelay.fixture.component",
    "--version", "1.2.3",
    "--install-location", "/",
    "--ownership", "recommended",
    packagePath,
  ]);
  runNative("/usr/sbin/pkgutil", ["--expand", packagePath, expandedPath]);
  const packageInfo = readFileSync(join(expandedPath, "PackageInfo"), "utf8");
  assert.match(packageInfo, /install-location="\/"/);
  assert.doesNotMatch(packageInfo, /<relocate(?:\s|>)/i);
  assert.doesNotMatch(packageInfo, /<bundle-version(?:\s|>)/i);
  const payloadFiles = runNative("/usr/sbin/pkgutil", ["--payload-files", packagePath]).split(/\r?\n/);
  for (const bundle of [manager, hal, nested]) {
    const expected = relative(packageRoot, bundle).split("/").join("/");
    assert.ok(payloadFiles.some(path => {
      const normalized = path.replace(/^\.\//, "");
      return normalized === expected || normalized.startsWith(`${expected}/`);
    }), `missing fixed payload path ${expected}; payload entries: ${payloadFiles.slice(0, 8).join(" | ")}`);
  }
  for (const filename of DOCUMENTS) {
    const expected = relative(packageRoot, join(manager, "Contents/Resources", filename));
    assert.ok(payloadFiles.some(path => path.replace(/^\.\//, "") === expected), `missing installed document ${filename}`);
  }
  const distributionPath = join(root, "distribution.xml");
  writeFileSync(distributionPath, `<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>MixRelay fixture</title>
  <readme file="INSTALLER-README.rtf" mime-type="text/rtf"/>
  <options customize="never"/>
  <choices-outline><line choice="default"/></choices-outline>
  <choice id="default" visible="false"><pkg-ref id="io.mixrelay.fixture.component"/></choice>
  <pkg-ref id="io.mixrelay.fixture.component" version="1.2.3">fixture.component.pkg</pkg-ref>
  <pkg-ref id="io.mixrelay.fixture.component">
    <must-close>
      <app id="io.mixrelay.driver.uninstaller"/>
    </must-close>
  </pkg-ref>
</installer-gui-script>
`);
  const productPath = join(root, "fixture.pkg");
  const expandedProduct = join(root, "expanded-product");
  runNative("/usr/bin/productbuild", ["--distribution", distributionPath, "--resources", resourcesPath, "--package-path", root, productPath]);
  runNative("/usr/sbin/pkgutil", ["--expand-full", productPath, expandedProduct]);
  for (const filename of DOCUMENTS) {
    const expandedDocument = join(expandedProduct, "fixture.component.pkg/Payload", relative(packageRoot, manager), "Contents/Resources", filename);
    assert.deepEqual(readFileSync(expandedDocument), readFileSync(join(manager, "Contents/Resources", filename)));
  }
  assert.deepEqual(readdirSync(join(expandedProduct, "Resources")).sort(), LANGUAGES.map(({language}) => `${language}.lproj`).sort());
  const installedNotice = readFileSync(join(expandedProduct, "fixture.component.pkg/Payload", relative(packageRoot, manager), "Contents/Resources/LICENSE.txt"));
  const expandedResources = Object.fromEntries(Object.keys(expectedProductResources)
    .map(filename => [filename, readFileSync(join(expandedProduct, "Resources", filename))]));
  assert.deepEqual(expandedResources, expectedProductResources);
  assertCopyrightNotice(expandedResources, installedNotice);
  assertInstallerReadme(expandedResources);
  assert.match(readFileSync(join(expandedProduct, "Distribution"), "utf8"), /<readme[^>]*file="INSTALLER-README\.rtf"/);
  assert.doesNotMatch(readFileSync(join(expandedProduct, "Distribution"), "utf8"), /<license\b/);
  const expandedDistribution = readFileSync(join(expandedProduct, "Distribution"), "utf8");
  const closeApps = expandedDistribution.match(/<must-close>([\s\S]*?)<\/must-close>/);
  assert.ok(closeApps, "productbuild preserves native close requirements in the product archive");
  assert.deepEqual([...closeApps[1].matchAll(/<app\b[^>]*\bid="([^"]+)"/g)].map(match => match[1]), [
    "io.mixrelay.driver.uninstaller",
  ]);

});

test("an output path is checked without creating it, before any release work", (t) => {
  const fixtureRoot = fileURLToPath(new URL("../target/test-artifacts/", import.meta.url));
  mkdirSync(fixtureRoot, { recursive: true });
  const root = mkdtempSync(join(fixtureRoot, "mixrelay-release-output-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const missing = join(root, "missing/output");
  checkPackageOutput(missing);
  assert.equal(fs.existsSync(missing), false);
  const real = join(root, "real");
  mkdirSync(real);
  checkPackageOutput(real);
  // A path through a symlink, such as /tmp on macOS, fails here and not after signing.
  const alias = join(root, "alias");
  symlinkSync(real, alias);
  assert.throws(() => checkPackageOutput(join(alias, "output")), /--output path must not contain symlinks/);
  assert.throws(() => checkPackageOutput(alias), /--output path must not contain symlinks/);
  writeFileSync(join(real, "left-over"), "");
  assert.throws(() => checkPackageOutput(real), /--output directory must be empty/);
  assert.throws(() => checkPackageOutput("relative/output"), /absolute/);
});
