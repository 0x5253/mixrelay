import assert from 'node:assert/strict';
import test from 'node:test';
import { parseOsRelease, versionAtLeast, validateEnvironment, validateCodeCheckEnvironment } from '../core/environment-policy.mjs';
import { qualificationFixture } from './qualification.fixture.mjs';

test('Linux release fields and numeric dependency floors preserve distro identity and version boundaries', t => {
  assert.deepEqual(parseOsRelease('ID=ubuntu\nVERSION_ID="26.04"\nPRETTY_NAME="Ubuntu 26.04 LTS"\n'),
    { ID: 'ubuntu', VERSION_ID: '26.04', PRETTY_NAME: 'Ubuntu 26.04 LTS' });
  assert.ok(versionAtLeast('1.2', '1.2.0'));
  assert.ok(versionAtLeast('1.10.0', '1.2.0'));
  assert.equal(versionAtLeast('1.1.99', '1.2.0'), false);
  assert.equal(versionAtLeast('unknown', '1.2.0'), false);
  const f = qualificationFixture(t);
  const target = f.loaded.contract.qualificationTargets.find(t => t.id === 'ubuntu-26-04-arm64');
  const environment = f.reports.find(r => r.targetId === target.id).environment;
  validateEnvironment({ ...environment, version: '26.04.1' }, target, f.loaded.contract);
  for (const changed of [{ distro: 'debian' }, { version: '26.10' }, { packageInventorySha256: '' },
    { dependencies: { ...environment.dependencies, pipewire: '1.1.99' } }]) {
    assert.throws(() => validateEnvironment({ ...environment, ...changed }, target, f.loaded.contract));
  }
});

test('source CI verifies its declared CPU and distro while keeping container checks separate from native qualification', t => {
  const f = qualificationFixture(t);
  const environment = { ...f.reports.find(r => r.targetId === 'debian-13-arm64').environment, kind: 'container' };
  validateCodeCheckEnvironment(environment, 'debian-arm64', f.loaded.contract);
  assert.throws(() => validateCodeCheckEnvironment(environment, 'debian-x64', f.loaded.contract));
  assert.throws(() => validateCodeCheckEnvironment({ ...environment, version: '12' }, 'debian-arm64', f.loaded.contract));
  assert.throws(() => validateCodeCheckEnvironment({ ...environment, distro: 'ubuntu' }, 'debian-arm64', f.loaded.contract));
  const mac = f.reports[0].environment;
  validateCodeCheckEnvironment(mac, 'macos-15-arm64', f.loaded.contract);
  assert.throws(() => validateCodeCheckEnvironment({ ...mac, translated: true }, 'macos-15-arm64', f.loaded.contract));
});

test('a hosted macOS major-version match cannot bypass the deployment minimum', t => {
  const f = qualificationFixture(t);
  const target = f.loaded.contract.qualificationTargets.find(t => t.id === 'macos-15-arm64');
  const observed = f.reports.find(r => r.targetId === target.id).environment;
  assert.throws(() => validateEnvironment({ ...observed, version: '14.9.9' }, target, f.loaded.contract), /Wrong OS version/);
  validateEnvironment({ ...observed, version: '15.0.0' }, target, f.loaded.contract);
  const raisedPatchMinimum = { ...f.loaded.contract, minimums: { ...f.loaded.contract.minimums, macos: '15.1' } };
  assert.throws(() => validateEnvironment({ ...observed, version: '15.0.9' }, target, raisedPatchMinimum), /Unsupported macOS/);
});
