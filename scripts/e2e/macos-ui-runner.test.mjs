import assert from 'node:assert/strict';
import test from 'node:test';
import { designatedRequirement, manualScreenCapturePolicy, prepareMacosUiRunner, runMacosUi, setEvidenceDirectory } from './macos-ui-runner.mjs';

const hostedRequirement = '# designated => cdhash H"32ba2e5952d60363e6b223f989834320e067bc61"\n';
const capture = { BlueprintName: 'InstalledUITests', IsUITestBundle: true,
  PreferredScreenCaptureFormat: 'screenshots', SystemAttachmentLifetime: 'keepNever' };

test('the native XCTest default video policy is rejected before the audio session', () => {
  assert.throws(() => manualScreenCapturePolicy({ InstalledUITests: { ...capture,
    PreferredScreenCaptureFormat: 'screenRecording', SystemAttachmentLifetime: 'deleteOnSuccess' } }), /video must be disabled/);
  assert.throws(() => manualScreenCapturePolicy({ InstalledUITests: { ...capture,
    SystemAttachmentLifetime: 'deleteOnSuccess' } }), /Automatic XCTest capture must be disabled/);
  assert.equal(manualScreenCapturePolicy({ InstalledUITests: capture }).targets.length, 1);
  assert.throws(() => manualScreenCapturePolicy({}), /actual UI test target/);
});

test('real hosted ad-hoc output is accepted on either codesign output stream', () => {
  for (const stream of ['stdout', 'stderr']) {
    assert.equal(designatedRequirement({ [stream]: hostedRequirement }),
      'cdhash H"32ba2e5952d60363e6b223f989834320e067bc61"');
  }
  assert.throws(() => designatedRequirement({ stderr: 'Executable=/runner\n' }), /one designated requirement/);
  assert.throws(() => designatedRequirement({ stdout: hostedRequirement, stderr: hostedRequirement }), /one designated requirement/);
});

test('the signed runner and Accessibility grant are prepared before installing the product', () => {
  const calls = [], files = new Map();
  let reads = 0;
  prepareMacosUiRunner('/tools', {
    runCommand(program, args) {
      calls.push([program, args]);
      if (program === 'plutil') return { stdout: args.includes('json')
        ? JSON.stringify({ InstalledUITests: capture }) : 'io.mixrelay.qualification.ui.xctrunner\n' };
      if (program === 'codesign' && args[0] === '-d') return { stdout: hostedRequirement, stderr: 'Executable=/runner\n' };
      if (program === 'launchctl') return { stdout: `state = running\n pid = ${++reads}\n` };
      return { stdout: '', stderr: '' };
    },
    read: () => Buffer.from([0xfa, 0xde]),
    list: () => ['InstalledUI_macosx.xctestrun', 'Debug'],
    write: (path, data) => files.set(path, data),
  });
  assert.equal(calls[0][1][0], 'build-for-testing');
  const sign = calls.findIndex(([p, args]) => p === 'codesign' && args[0] === '--force');
  const grant = calls.findIndex(([p]) => p === 'sudo');
  assert.ok(sign > 0 && grant > sign);
  const compile = calls.findIndex(([p]) => p === 'csreq');
  assert.equal(calls[compile][1][1], '=cdhash H"32ba2e5952d60363e6b223f989834320e067bc61"');
  assert.deepEqual(calls[compile + 1], ['codesign', ['--verify', '--strict', '-R', '/tools/runner.csreq', calls[sign][1].at(-1)]]);
  assert.ok(compile + 1 < grant);
  const sql = calls[grant][1].at(-1);
  assert.match(sql, /'io\.mixrelay\.qualification\.ui\.xctrunner',0,2,4,1,X'fade'/);
  assert.match(sql, /'kTCCServiceAccessibility'/);
  assert.match(sql, /'kTCCServiceScreenCapture'/);
  assert.doesNotMatch(sql, /kTCCServiceMicrophone|kTCCServiceAudioCapture/);
  const expected = JSON.parse(files.get('/tools/prepared-runner.json'));
  assert.equal(expected.path, calls[sign][1].at(-1));
  assert.equal(expected.bundle, 'io.mixrelay.qualification.ui.xctrunner');
  assert.equal(JSON.parse(files.get('/tools/capture-policy.json')).targets[0].preferredScreenCaptureFormat, 'screenshots');
  assert.ok(expected.requirement.includes('cdhash'));
  assert.deepEqual(calls.filter(([p]) => p === 'xcodebuild').map(([, args]) => args[0]),
    ['build-for-testing']);
  assert.deepEqual(JSON.parse(files.get('/tools/tcc-restart.json')), { before: '1', after: '2' });
  assert.ok(calls.some(([p, args]) => p === 'sudo' && args.join(' ') === '-n launchctl kickstart -k -p system/com.apple.tccd.system'));
  assert.ok(calls.every(([, args]) => !args.includes('killall')));
  assert.ok(calls.slice(grant + 1).every(([p]) => p !== 'codesign'));
});

test('both installed modes reuse the same signed code and change only the XCTest evidence environment', () => {
  const calls = [], files = new Map();
  const prepared = { bundle: 'io.mixrelay.qualification.ui.xctrunner',
    path: '/tools/DerivedData/Build/Products/Debug/InstalledUITests-Runner.app',
    template: '/tools/DerivedData/Build/Products/InstalledUI_macosx.xctestrun', requirement: 'cdhash H"abc"' };
  const template = { TestConfigurations: [{ TestTargets: [{ ...capture, EnvironmentVariables: { MIXRELAY_UI_EVIDENCE: '/unused', OTHER: 'retained' },
    TestHostPath: '__TESTROOT__/Debug/InstalledUITests-Runner.app' }] }], __xctestrun_metadata__: { FormatVersion: 2 } };
  for (const mode of ['fresh', 'upgrade']) runMacosUi(`/evidence/${mode}`, '/tools', {
    read: () => JSON.stringify(prepared),
    write: (path, data) => files.set(path, data),
    runCommand(program, args) {
      calls.push([program, args]);
      return { stdout: program === 'plutil' && args.includes('json') ? JSON.stringify(template) : '' };
    },
  });
  for (const mode of ['fresh', 'upgrade']) {
    const configuration = `/tools/DerivedData/Build/Products/installed-${mode}.xctestrun`;
    const stored = JSON.parse(files.get(configuration));
    assert.equal(stored.TestConfigurations[0].TestTargets[0].EnvironmentVariables.MIXRELAY_UI_EVIDENCE, `/evidence/${mode}`);
    assert.equal(stored.TestConfigurations[0].TestTargets[0].EnvironmentVariables.OTHER, 'retained');
    assert.equal(stored.TestConfigurations[0].TestTargets[0].TestHostPath, '__TESTROOT__/Debug/InstalledUITests-Runner.app');
    assert.deepEqual(JSON.parse(files.get(`/evidence/${mode}/installed-ui-runner-expected.json`)), prepared);
    assert.ok(calls.some(([p, args]) => p === 'xcodebuild' && args.includes(configuration)));
  }
  assert.deepEqual(calls.filter(([p]) => p === 'xcodebuild').map(([, args]) => args[0]), ['test-without-building', 'test-without-building']);
  assert.ok(calls.filter(([p]) => p === 'xcodebuild').every(([, args]) => args.includes('CODE_SIGNING_ALLOWED=NO')));
  assert.ok(calls.every(([p, args]) => p !== 'sudo' && p !== 'launchctl' && !args.includes('--force')));
});

test('an unconfirmed TCC restart fails preparation instead of starting product tests', () => {
  const files = new Map();
  assert.throws(() => prepareMacosUiRunner('/tools', {
    runCommand(program, args) {
      if (program === 'plutil') return { stdout: 'io.mixrelay.qualification.ui.xctrunner\n' };
      if (program === 'codesign' && args[0] === '-d') return { stdout: hostedRequirement };
      return { stdout: 'pid = 1\n' };
    },
    read: () => Buffer.from([0xfa]), write: (path, data) => files.set(path, data),
  }), /must restart before product installation/);
  assert.equal(files.has('/tools/prepared-runner.json'), false);
  assert.throws(() => setEvidenceDirectory({ EnvironmentVariables: { unrelated: 'same' } }, '/fresh'), /lost its evidence environment/);
});

test('an unexpected generated identity fails before signing, granting, or running tests', () => {
  const calls = [];
  assert.throws(() => prepareMacosUiRunner('/tools', {
    runCommand(program) {
      calls.push(program);
      return { stdout: 'io.mixrelay.qualification.ui\n' };
    },
  }), /io.mixrelay.qualification.ui/);
  assert.deepEqual(calls, ['xcodebuild', 'plutil']);
});
