// Run in an isolated Node process: only installation, host observation and removal are replaced.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { mock, test } from 'node:test';
import { qualificationFixture } from './qualification.fixture.mjs';
import * as packageApi from './package.mjs';
import * as collection from './collect-macos.mjs';
import * as environment from './environment.mjs';

test('fresh installation is removed when a check after the installer rejects it', async t => {
  const f = qualificationFixture(t);
  const output = join(f.directory, 'rejected-install');
  let installations = 0, removals = 0;
  mock.module('./package.mjs', { exports: {
    ...packageApi,
    requireDisposableHost() {},
    installAndVerify(_artifact, _definition, _directory, _output, _os, { onInstalled }) {
      installations++;
      onInstalled();
      throw new Error('Installed executable differs from candidate');
    },
    launchInstalled() { assert.fail('Rejected installed executable must not launch'); },
  } });
  mock.module('./environment.mjs', { exports: {
    ...environment, observeEnvironment: () => f.reports[0].environment,
  } });
  mock.module('./collect-macos.mjs', { exports: { ...collection, collectMacos() {} } });
  mock.module('./removal.mjs', { exports: { removeInstalled(os, format, directory) {
    assert.equal(installations, 1);
    assert.equal(os, 'macos'); assert.equal(format, 'pkg');
    removals++;
    writeFileSync(join(directory, 'removal.log'), 'Synthetic removal completed\n');
  } } });
  const { qualifyMode } = await import('./installed.mjs');
  const report = await qualifyMode({ targetId: f.reports[0].targetId,
    candidate: f.candidate, directory: f.directory, output, disposable: true });
  assert.equal(report.status, 'failed');
  assert.match(report.scenarios.find(item => item.id === 'PKG-01').error,
    /differs from candidate/);
  assert.equal(removals, 1);
  assert.equal(report.scenarios.find(item => item.id === 'PKG-02').status, 'passed');
  assert.equal(report.cleanup, 'passed');
  assert.match(readFileSync(join(output, 'removal.log'), 'utf8'), /completed/);
});
