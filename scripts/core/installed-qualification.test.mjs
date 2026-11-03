import assert from 'node:assert/strict';
import test from 'node:test';
import { runQualificationMode } from './installed-qualification.mjs';

function fixture(failure) {
  const files = new Map(), lifecycle = [];
  const environment = {
    os: 'linux', arch: 'x64', distro: 'test', version: '1', kind: 'native-session',
    kernel: 'fixture', osRelease: 'ID=test', packageInventorySha256: 'a'.repeat(64),
    dependencies: { pipewire: '1.2.0', wireplumber: '0.5.0', glibc: '2.39.0' },
  };
  const target = { id: 'test', os: 'linux', arch: 'x64', distro: 'test', artifact: 'app', version: '1', versionMatch: 'major' };
  const artifact = { id: 'app', sha256: 'a'.repeat(64), size: 4 };
  const options = {
    targetId: 'test', directory: '/candidate', output: '/evidence', disposable: true,
    candidate: { version: '1.0.0', sourceRevision: 'b'.repeat(40), artifacts: [artifact] },
    loaded: { sha256: 'c'.repeat(64), contract: {
      qualificationTargets: [target], artifacts: [{ id: 'app', format: 'deb' }],
      minimums: { linux: { pipewire: '1.2.0', wireplumber: '0.5.0', glibc: '2.39.0' } },
      scenarios: ['PKG-01', 'APP-01', 'GRAPH-01', 'PKG-02'].map((id) => ({ id, required: true, scope: 'artifact' })),
    } },
  };
  const ports = {
    storage: {
      mkdir: () => {}, exists: () => false,
      read: () => Buffer.from('data'),
      write: (path, bytes, options) => { assert.equal(options.flag, 'wx'); assert.equal(files.has(path), false); files.set(path, bytes); },
    },
    context: {
      variables: {}, timestamp: () => '2026-11-02T00:00:00.000Z', now: () => 0, wait: async () => {}, log: () => {},
      observeEnvironment: () => { if (failure === 'prerequisite') throw new Error('Host observation failed'); return environment; },
    },
    artifacts: {
      validateCandidate: () => {}, artifactBytes: () => ({ sha256: artifact.sha256, size: artifact.size }),
    },
    runtime: { engine: () => ({}), driver: () => async () => ({}) },
    installation: {
      requireDisposableHost: () => {},
      installAndVerify: (_artifact, _definition, _directory, _output, _os, { onInstalled }) => { lifecycle.push('install'); onInstalled(); if (failure === 'verify') throw new Error('Installed binary verification failed'); },
      launchInstalled: async () => { lifecycle.push('launch'); throw new Error('Runtime startup failed'); },
      removeInstalled: () => { lifecycle.push('remove'); },
    },
    ui: { finishAfterCleanup: async (_ui, cleanup) => cleanup() },
    audio: {}, upgrade: {},
    diagnostics: { collectLinux: () => lifecycle.push('diagnose') },
  };
  return { options, ports, lifecycle, files };
}

for (const failure of ['verify', 'launch']) {
  test(`installed payload is removed after ${failure} failure and both outcomes remain in the report`, async () => {
    const f = fixture(failure);
    const report = await runQualificationMode(f.options, f.ports);
    assert.equal(report.status, 'failed');
    assert.equal(report.scenarios.find((item) => item.id === 'PKG-02').status, 'passed');
    assert.equal(f.lifecycle.filter((action) => action === 'remove').length, 1);
    assert.ok(f.lifecycle.indexOf('diagnose') < f.lifecycle.indexOf('remove'));
    assert.match(report.error, failure === 'verify' ? /Installation or installed artifact identity failed/ : /Runtime startup failed/);
    assert.ok(f.files.has('/evidence/test.mode.json'));
    if (failure === 'verify') assert.equal(report.scenarios.find((item) => item.id === 'PKG-01').status, 'failed');
  });
}

test('a failed prerequisite records blocked scenarios without installing or removing a payload', async () => {
  const f = fixture('prerequisite');
  const report = await runQualificationMode(f.options, f.ports);
  assert.equal(report.error, 'Host observation failed');
  assert.equal(report.status, 'failed');
  assert.deepEqual(f.lifecycle, ['diagnose']);
  assert.ok(report.scenarios.every((scenario) => scenario.status === 'blocked'));
});
