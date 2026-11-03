import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { prepareHostedMacos } from './prepare-macos-host.mjs';

const crashed = 'state = spawn scheduled\nsuccessive crashes = 15\nlast terminating signal = Abort trap: 6\n';
const timeout = { status: null, signal: 'SIGKILL', error: Object.assign(new Error('timed out'), { code: 'ETIMEDOUT' }) };

function fixture(t, { first = { status: 0, stdout: '4096' }, state = crashed, after = { status: 0, stdout: '4096' }, arch = 'x64', disabledState = 'true', indexingBefore = 'Indexing and searching disabled.', indexingAfter = 'Indexing and searching disabled.' } = {}) {
  const output = mkdtempSync(join(tmpdir(), 'mr-host-'));
  t.after(() => rmSync(output, { recursive: true, force: true }));
  const calls = []; let probes = 0, inspections = 0, indexingReads = new Map();
  const options = { platform: 'darwin', arch, uid: 501, env: { GITHUB_ACTIONS: 'true', ImageOS: 'macos26' },
    command(program, args, options) {
      calls.push({ program, args, options });
      if (program === 'sw_vers') return { status: 0, stdout: '26.6.1\n' };
      if (program === 'mdutil') {
        const volume = args.at(-1);
        const reads = (indexingReads.get(volume) ?? 0) + 1;
        indexingReads.set(volume, reads);
        return { status: 0, stdout: reads === 1 ? indexingBefore : indexingAfter };
      }
      if (program.endsWith('icon-probe')) return ++probes === 1 ? first : after;
      if (args[0] === 'print') return ++inspections === 1 ? { status: 0, stdout: state } : { status: 113, stderr: 'Could not find service' };
      if (args[0] === 'print-disabled') return { status: 0, stdout: `"com.apple.iconservices.iconservicesagent" => ${disabledState}\n` };
      return { status: 0, stdout: '' };
    } };
  return { output, calls, run: () => prepareHostedMacos(output, options), options };
}

test('a healthy native icon probe leaves the hosted services unchanged', t => {
  const f = fixture(t);
  assert.equal(f.run().repaired, false);
  assert.ok(f.calls.every(call => !['bootout', 'disable'].includes(call.args[0])));
  assert.equal(JSON.parse(readFileSync(join(f.output, 'host-preparation.json'))).version, '26.6.1');
});

test('only a timed out icon request with the matching crash loop repairs the disposable image and rechecks pixels', t => {
  const f = fixture(t, { first: timeout });
  assert.equal(f.run().repaired, true);
  assert.deepEqual(f.calls.filter(call => ['bootout', 'disable'].includes(call.args[0])).map(call => call.args),
    [['bootout', 'gui/501/com.apple.iconservices.iconservicesagent'], ['disable', 'gui/501/com.apple.iconservices.iconservicesagent']]);
  assert.equal(f.calls.filter(call => call.program.endsWith('icon-probe')).length, 2);
  assert.ok(f.calls.every(call => call.options.timeout > 0 && call.options.killSignal === 'SIGKILL'));
});

test('unrelated probe failures and unconfirmed crash signatures fail without changing services', t => {
  for (const options of [
    { first: { status: 1, stderr: 'invalid image' } },
    { first: timeout, state: 'state = running\nsuccessive crashes = 0\n' },
  ]) {
    const f = fixture(t, options);
    assert.throws(f.run);
    assert.ok(f.calls.every(call => !['bootout', 'disable'].includes(call.args[0])));
  }
});

test('a failed after-repair native probe remains a preparation failure', t => {
  const f = fixture(t, { first: timeout, after: timeout });
  assert.throws(f.run, /timed out/);
  assert.equal(JSON.parse(readFileSync(join(f.output, 'host-preparation.json'))).repaired, false);
});

test('launchctl disabled-state words are accepted but an enabled agent remains a failure', t => {
  const f = fixture(t, { first: timeout, disabledState: 'disabled' });
  assert.equal(f.run().repaired, true);
  for (const disabledState of ['false', 'enabled']) {
    const unconfirmed = fixture(t, { first: timeout, disabledState });
    assert.throws(unconfirmed.run);
    assert.equal(unconfirmed.calls.filter(call => call.program.endsWith('icon-probe')).length, 1);
  }
});

test('other CPUs and non-disposable sessions never change the hosted service', t => {
  const f = fixture(t, { arch: 'arm64' });
  assert.equal(f.run().repaired, false);
  assert.deepEqual(f.calls.map(call => call.program), ['sw_vers', 'mdutil', 'mdutil']);
  assert.throws(() => prepareHostedMacos(f.output, { ...f.options, env: {} }), /disposable/);
});

test('disposable audio hosts pause active indexing before native preparation and retain the observed states', t => {
  const f = fixture(t, { indexingBefore: 'Indexing enabled.', indexingAfter: 'Indexing and searching disabled.' });
  const report = f.run();
  assert.deepEqual(report.spotlight.volumes, ['/', '/System/Volumes/Data'].map(volume => ({ volume, changed: true, activityBefore: 'enabled', activityAfter: 'disabled' })));
  const pause = f.calls.findIndex(call => call.program === 'sudo' && call.args.includes('mdutil'));
  const compile = f.calls.findIndex(call => call.program === 'xcrun');
  assert.ok(pause >= 0 && pause < compile);
  assert.deepEqual(f.calls[pause].args, ['-n', 'mdutil', '-d', '/']);
  assert.equal(JSON.parse(readFileSync(join(f.output, 'host-preparation.json'))).spotlight.volumes[1].activityAfter, 'disabled');
});

test('unrecognized or still-active indexing fails preparation without disabling unrelated services', t => {
  for (const options of [{ indexingBefore: 'unknown state' }, { indexingBefore: 'Indexing enabled.', indexingAfter: 'Indexing enabled.' }, { indexingBefore: 'Indexing enabled.', indexingAfter: 'Indexing disabled.' }]) {
    const f = fixture(t, options);
    assert.throws(f.run, /Spotlight/);
    assert.ok(f.calls.every(call => !['bootout', 'disable'].includes(call.args[0])));
    assert.ok(f.calls.every(call => call.program !== 'xcrun'));
  }
});

test('indexing off alone still requires activity suppression on both system and data volumes', t => {
  const f = fixture(t, { indexingBefore: 'Indexing disabled.' });
  const report = f.run();
  assert.deepEqual(f.calls.filter(call => call.program === 'sudo').map(call => call.args),
    [['-n', 'mdutil', '-d', '/'], ['-n', 'mdutil', '-d', '/System/Volumes/Data']]);
  assert.ok(report.spotlight.volumes.every(volume => volume.activityBefore === 'indexing-disabled' && volume.activityAfter === 'disabled'));
});
