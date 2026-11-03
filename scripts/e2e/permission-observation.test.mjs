import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { permissionObservation } from './permission-observation.mjs';

test('an acknowledged pause excludes scans during PCM and resumes after success or PCM failure', async t => {
  for (const failure of [false, true]) {
    const directory = mkdtempSync(join(tmpdir(), 'mr-observation-'));
    t.after(() => rmSync(directory, { recursive: true, force: true }));
    let observing = true;
    const events = [];
    const boundary = permissionObservation(directory, () => true, { wait: async () => {
      const command = JSON.parse(readFileSync(join(directory, 'permission-observation.command.json')));
      observing = command.phase === 'observing';
      events.push(command.phase);
      writeFileSync(join(directory, 'permission-observation.ack.json'), JSON.stringify({ ...command, at: new Date().toISOString() }));
    } });
    const work = boundary(() => {
      assert.equal(observing, false);
      events.push('PCM');
      if (failure) throw new Error('6724 missingFrames');
      return 144000;
    });
    if (failure) await assert.rejects(work, /6724 missingFrames/);
    else assert.equal(await work, 144000);
    assert.equal(observing, true);
    assert.deepEqual(events, ['paused', 'PCM', 'observing']);
    const receipt = JSON.parse(readFileSync(join(directory, 'native-permission-observation.json')));
    assert.equal(receipt.paused.phase, 'paused');
    assert.equal(receipt.resumed.phase, 'observing');
    assert.notEqual(receipt.paused.generation, receipt.resumed.generation);
  }
});

test('a stale acknowledgement or dead observer cannot start measurement', async t => {
  for (const alive of [true, false]) {
    const directory = mkdtempSync(join(tmpdir(), 'mr-observation-refused-'));
    t.after(() => rmSync(directory, { recursive: true, force: true }));
    writeFileSync(join(directory, 'permission-observation.ack.json'), JSON.stringify({ generation: 'old', phase: 'paused' }));
    let clock = 0, measurements = 0;
    const boundary = permissionObservation(directory, () => alive, { now: () => clock, wait: async ms => { clock += ms; } });
    await assert.rejects(boundary(() => { measurements++; }), /permission observer/);
    assert.equal(measurements, 0);
  }
});

test('a failed resume retains both PCM and observer errors', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-observation-resume-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  let clock = 0;
  const boundary = permissionObservation(directory, () => true, { now: () => clock, wait: async ms => {
    const command = JSON.parse(readFileSync(join(directory, 'permission-observation.command.json')));
    if (command.phase === 'paused') writeFileSync(join(directory, 'permission-observation.ack.json'), JSON.stringify({ ...command, at: new Date().toISOString() }));
    clock += ms;
  } });
  await assert.rejects(boundary(() => { throw new Error('PCM failed'); }), error => {
    assert.equal(error.errors.length, 2);
    assert.match(error.message, /PCM failed.*did not acknowledge observing/);
    return true;
  });
});

test('additional source measurements retain separate pause receipts without replacing required PCM evidence', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-observation-receipts-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const boundary = permissionObservation(directory, () => true, { wait: async () => {
    const command = JSON.parse(readFileSync(join(directory, 'permission-observation.command.json')));
    writeFileSync(join(directory, 'permission-observation.ack.json'), JSON.stringify({ ...command, at: new Date().toISOString() }));
  } });
  await boundary(() => {});
  const original = readFileSync(join(directory, 'native-permission-observation.json'), 'utf8');
  const generations = [];
  for (const order of ['silent-audible', 'audible-silent']) {
    const receiptDirectory = join(directory, order); mkdirSync(receiptDirectory);
    await boundary(() => {}, { receiptDirectory });
    generations.push(JSON.parse(readFileSync(join(receiptDirectory, 'native-permission-observation.json'))).paused.generation);
  }
  assert.notEqual(generations[0], generations[1]);
  assert.equal(readFileSync(join(directory, 'native-permission-observation.json'), 'utf8'), original);
});
