import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

function run(program, args, { quiet = false } = {}) {
  const result = spawnSync(program, args, { encoding: 'utf8', timeout: program === 'xcodebuild' ? 600000 : 30000,
    maxBuffer: 16 * 1024 * 1024 });
  if (!quiet) process.stdout.write(result.stdout ?? '');
  process.stderr.write(result.stderr ?? '');
  if (result.error || result.status !== 0) throw result.error ?? new Error(`${program} exited ${result.status}`);
  return result;
}

export function designatedRequirement({ stdout = '', stderr = '' }) {
  const matches = [...`${stdout}\n${stderr}`.matchAll(/^(?:#\s*)?designated => (.+)\r?$/gm)];
  assert.equal(matches.length, 1, 'Signed XCTest runner must have one designated requirement');
  return matches[0][1].trim();
}

function servicePid(value) {
  return value.match(/^\s*pid = ([1-9][0-9]*)\s*$/m)?.[1];
}

export function manualScreenCapturePolicy(configuration) {
  const targets = [];
  function visit(value) {
    if (!value || typeof value !== 'object') return;
    if (value.IsUITestBundle === true) {
      assert.equal(value.PreferredScreenCaptureFormat, 'screenshots', 'Continuous XCTest video must be disabled');
      assert.equal(value.SystemAttachmentLifetime, 'keepNever', 'Automatic XCTest capture must be disabled');
      targets.push({ name: value.BlueprintName, preferredScreenCaptureFormat: value.PreferredScreenCaptureFormat,
        systemAttachmentLifetime: value.SystemAttachmentLifetime });
    }
    for (const item of Object.values(value)) visit(item);
  }
  visit(configuration);
  assert.ok(targets.length > 0, 'An actual UI test target is required');
  return { targets };
}

export function prepareMacosUiRunner(tools, { runCommand = run, read = readFileSync,
  write = writeFileSync, list = readdirSync } = {}) {
  const build = ['-project', join(tools, 'MixRelayInstalledUI.xcodeproj'), '-scheme', 'InstalledUI',
    '-destination', 'platform=macOS', '-derivedDataPath', join(tools, 'DerivedData'),
    '-parallel-testing-enabled', 'NO', 'CODE_SIGNING_ALLOWED=NO'];
  runCommand('xcodebuild', ['build-for-testing', ...build]);
  const runner = join(tools, 'DerivedData/Build/Products/Debug/InstalledUITests-Runner.app');
  const bundle = runCommand('plutil', ['-extract', 'CFBundleIdentifier', 'raw', '-o', '-', join(runner, 'Contents/Info.plist')]).stdout.trim();
  assert.equal(bundle, 'io.mixrelay.qualification.ui.xctrunner');
  runCommand('codesign', ['--force', '--deep', '--sign', '-', runner]);
  runCommand('codesign', ['--verify', '--deep', '--strict', runner]);
  const requirement = designatedRequirement(runCommand('codesign', ['-d', '-r-', runner]));
  const csreq = join(tools, 'runner.csreq');
  runCommand('csreq', ['-r', `=${requirement}`, '-b', csreq]);
  runCommand('codesign', ['--verify', '--strict', '-R', csreq, runner]);
  const bytes = read(csreq).toString('hex');
  assert.match(bytes, /^(?:[0-9a-f]{2})+$/);
  // This grant is confined to the disposable hosted session and to the exact
  // generated runner's code requirement. test-without-building preserves it.
  const sql = ['kTCCServiceAccessibility', 'kTCCServiceScreenCapture'].map(service => `INSERT OR REPLACE INTO access
    (service,client,client_type,auth_value,auth_reason,auth_version,csreq,
     indirect_object_identifier_type,indirect_object_identifier,flags,last_modified)
    VALUES ('${service}','${bundle}',0,2,4,1,X'${bytes}',0,'UNUSED',0,strftime('%s','now'));`).join('\n');
  runCommand('sudo', ['-n', 'sqlite3', '/Library/Application Support/com.apple.TCC/TCC.db', sql]);
  const service = 'system/com.apple.tccd.system';
  const before = servicePid(runCommand('launchctl', ['print', service]).stdout);
  runCommand('sudo', ['-n', 'launchctl', 'kickstart', '-k', '-p', service]);
  const after = servicePid(runCommand('launchctl', ['print', service]).stdout);
  assert.ok(after && after !== before, 'The hosted TCC daemon must restart before product installation');
  write(join(tools, 'tcc-restart.json'), `${JSON.stringify({ before, after }, null, 2)}\n`);
  const products = join(tools, 'DerivedData/Build/Products');
  const templates = list(products).filter(name => name.endsWith('.xctestrun'));
  assert.equal(templates.length, 1, 'One immutable XCTest configuration is required');
  const prepared = { bundle, path: runner, requirement, template: join(products, templates[0]) };
  const configuration = JSON.parse(runCommand('plutil', ['-convert', 'json', '-o', '-', prepared.template], { quiet: true }).stdout);
  write(join(tools, 'capture-policy.json'), `${JSON.stringify(manualScreenCapturePolicy(configuration), null, 2)}\n`);
  write(join(tools, 'prepared-runner.json'), `${JSON.stringify(prepared, null, 2)}\n`);
  return prepared;
}

export function setEvidenceDirectory(configuration, output) {
  let changed = 0;
  function visit(value) {
    if (!value || typeof value !== 'object') return;
    for (const [key, item] of Object.entries(value)) {
      if (key === 'MIXRELAY_UI_EVIDENCE') { value[key] = output; changed++; }
      else visit(item);
    }
  }
  visit(configuration);
  assert.ok(changed > 0, 'Prepared XCTest configuration lost its evidence environment');
  return configuration;
}

export function runMacosUi(output, tools, { runCommand = run, read = readFileSync, write = writeFileSync } = {}) {
  const prepared = JSON.parse(read(join(tools, 'prepared-runner.json'), 'utf8'));
  assert.equal(prepared.bundle, 'io.mixrelay.qualification.ui.xctrunner');
  assert.equal(prepared.path, join(tools, 'DerivedData/Build/Products/Debug/InstalledUITests-Runner.app'));
  runCommand('codesign', ['--verify', '--deep', '--strict', prepared.path]);
  runCommand('codesign', ['--verify', '--strict', '-R', join(tools, 'runner.csreq'), prepared.path]);
  const template = JSON.parse(runCommand('plutil', ['-convert', 'json', '-o', '-', prepared.template], { quiet: true }).stdout);
  manualScreenCapturePolicy(template);
  const configuration = join(tools, 'DerivedData/Build/Products', `installed-${basename(output)}.xctestrun`);
  write(configuration, `${JSON.stringify(setEvidenceDirectory(template, output))}\n`);
  runCommand('plutil', ['-convert', 'xml1', configuration]);
  write(join(output, 'installed-ui-runner-expected.json'), `${JSON.stringify(prepared, null, 2)}\n`);
  // No build, re-signing, privacy grant or daemon restart while the product runs.
  runCommand('xcodebuild', ['test-without-building', '-xctestrun', configuration,
    '-destination', 'platform=macOS', '-parallel-testing-enabled', 'NO',
    'CODE_SIGNING_ALLOWED=NO', '-resultBundlePath', join(output, 'installed-ui.xcresult')]);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  assert.equal(process.platform, 'darwin');
  assert.equal(process.env.GITHUB_ACTIONS, 'true', 'TCC setup requires a disposable hosted runner');
  if (process.argv[2] === '--prepare') prepareMacosUiRunner(resolve(process.argv[3]));
  else runMacosUi(resolve(process.argv[2]), resolve(process.argv[3]));
}
