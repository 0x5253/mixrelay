import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import test from 'node:test';
import { sourceComparisonPlan, sourceComparisonPcm, sourceOrders } from './source-comparison-plan.mjs';
import { analyseSourceComparison } from './source-comparison-analysis.mjs';
import { recordSourceComparison } from './source-comparison-recording.mjs';
import { prepareSourceComparison } from './prepare-source-comparison.mjs';
import { compareMacosSources } from './macos-source-comparison.mjs';
import { pcmHash } from './pcm-hash.mjs';

function fixture(order) {
  const plan = sourceComparisonPlan(order), source = sourceComparisonPcm(plan), frames = Math.ceil(plan.frames / 512) * 512;
  const capture = Buffer.alloc(frames * 8); source.copy(capture);
  const metrics = { sourceFrames: plan.frames, recordedFrames: frames, origin: 100000, hasOrigin: true,
    rate: 48000, channels: 2, bufferFrames: 512, timestampPeriod: 16384, inputCallbacks: frames / 512, outputCallbacks: frames / 512,
    invalidBuffers: 0, invalidTimestamps: 0, overflow: 0, droppedChunks: 0, activityProtection: true,
    completed: true, captureSaved: true, traceSaved: true, traceDisabled: true,
    nativeStatus: { create: 0, start: 0, stop: 0, destroy: 0 }, inputAuthorization: { initial: 3, final: 3, requested: false } };
  const rows = [];
  for (const side of ['output', 'input']) for (let first = 0; first < frames; first += 512)
    rows.push([side, first, 100000 + first, 10000000 + first * 1000, 512, 3, 5 + first * 1000, 8 + first * 1000]);
  return { plan, source, metrics, capture, rows };
}
const analyse = value => analyseSourceComparison(value.plan, value.source, value.metrics,
  `timebase,125,3\n${value.rows.map(row => row.join(',')).join('\n')}\n`, value.capture);

for (const order of sourceOrders) test(`${order}: covers both three-second inputs and marks silence identity unobservable`, () => {
  const data = fixture(order), result = analyse(data);
  assert.equal(result.passed, true);
  const silent = result.segments.find(row => row.kind === 'silent'), audible = result.segments.find(row => row.kind === 'audible');
  assert.equal(silent.zeroFrames, 144000);
  assert.equal(silent.sampleIdentityObservable, false);
  assert.equal(audible.zeroFrames, 0);
  assert.equal(audible.sampleIdentityObservable, true);
  assert.deepEqual(silent.input, { frames: 144000, missingFrames: 0, duplicateFrames: 0 });
});

test('all-zero silence cannot conceal a missing native timeline block', () => {
  const data = fixture('silent-audible');
  const removed = data.rows.find(row => row[0] === 'input' && row[1] === 10240);
  data.rows = data.rows.filter(row => row !== removed);
  // Compact the raw capture exactly as the native recorder does when an input callback never arrives.
  data.capture = Buffer.concat([data.capture.subarray(0, 10240 * 8), data.capture.subarray((10240 + 512) * 8)]);
  for (const row of data.rows) if (row[0] === 'input' && row[1] > 10240) row[1] -= 512;
  data.metrics.recordedFrames -= 512; data.metrics.inputCallbacks--;
  const result = analyse(data), silent = result.segments[0];
  assert.equal(result.passed, false);
  assert.equal(silent.wrongFrames, 0);
  assert.equal(silent.input.missingFrames, 512);
  assert.equal(result.discontinuities.input[0].frames, 512);
});

test('a nonzero leak into the silent input fails independently of audible source integrity', () => {
  const data = fixture('audible-silent'), frame = data.plan.segments[1].start + 16;
  data.capture.writeFloatLE(0.125, frame * 8 + 4);
  const result = analyse(data);
  assert.equal(result.passed, false);
  assert.equal(result.segments[0].passed, true);
  assert.equal(result.segments[1].wrongFrames, 1);
});

test('a 48-frame previous-period replay in audible PCM is retained as an exact failure range', () => {
  const data = fixture('audible-silent'), start = 30000;
  data.source.copy(data.capture, start * 8, (start - 16384) * 8, (start - 16384 + 48) * 8);
  const result = analyse(data);
  assert.equal(result.passed, false);
  assert.equal(result.segments[0].wrongFrames, 48);
  assert.deepEqual(result.segments[0].differences, [{ start, end: start + 48, recordedStart: start, matchesPreviousTimestampPeriod: true }]);
});

test('callback host time must advance even when the source is silent', () => {
  const data = fixture('silent-audible'), inputs = data.rows.filter(row => row[0] === 'input');
  inputs[20][3] = inputs[19][3];
  const result = analyse(data);
  assert.equal(result.passed, false);
  assert.equal(result.discontinuities.input[0].kind, 'host');
});

test('failed source diagnosis finalizes its separate recording and preserves both errors', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-source-recording-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  let state = 'stopped'; const actions = [];
  await assert.rejects(recordSourceComparison({ directory, recorderPath: 'recorder', revision: 7,
    api: async () => ({ state, path: 'take.wav' }),
    controlTake: async value => {
      actions.push(value.request.action); state = value.request.action === 'record' ? 'recording' : 'stopped';
      return { state, error: state === 'stopped' ? 'disk failure' : null };
    }, compare: async () => { throw new Error('native PCM mismatch'); } }), error => {
    assert.equal(error.errors.length, 2);
    assert.match(error.errors[0].message, /native PCM mismatch/);
    assert.match(error.errors[1].message, /disk failure/);
    return true;
  });
  assert.deepEqual(actions, ['record', 'stop']);
  assert.equal(state, 'stopped');
});

test('both fixed source orders run once with the same executable and retain a failed first result', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-source-orders-')), oldActions = process.env.GITHUB_ACTIONS;
  process.env.GITHUB_ACTIONS = 'true';
  t.after(() => { rmSync(directory, { recursive: true, force: true });
    if (oldActions === undefined) delete process.env.GITHUB_ACTIONS; else process.env.GITHUB_ACTIONS = oldActions; });
  const commands = [], receipts = [];
  const bundle = join(directory, 'tool.app'); mkdirSync(join(bundle, 'Contents/MacOS'), { recursive: true });
  writeFileSync(join(bundle, 'Contents/MacOS/MRSourceControl'), 'one executable');
  const execute = (program, args) => {
    if (program === 'pkill') { const error = new Error('already exited'); error.status = 1; throw error; }
    assert.equal(program, 'open');
    const output = args[args.indexOf('--stdout') + 1];
    if (args.includes('--authorize-input')) { writeFileSync(output, JSON.stringify({ inputAuthorization: { final: 3 } })); return; }
    const sourcePath = args[args.indexOf('--args') + 2], order = basename(dirname(sourcePath)), data = fixture(order);
    commands.push({ order, bundle: args[args.indexOf('--args') - 1] });
    assert.deepEqual(readFileSync(sourcePath), data.source);
    if (order === 'silent-audible') data.capture.writeFloatLE(0.125, 10000 * 8);
    writeFileSync(output, JSON.stringify(data.metrics));
    writeFileSync(args[args.indexOf('--stderr') + 1], `timebase,125,3\n${data.rows.map(row => row.join(',')).join('\n')}\n`);
    writeFileSync(args.at(-2), data.capture);
    const rows = [], paddedSource = Buffer.concat([data.source, Buffer.alloc(data.capture.length - data.source.length)]);
    for (let first = 0; first < data.metrics.recordedFrames; first += 512) {
      for (const render of [1, 0]) {
        const pcm = render ? paddedSource : data.capture;
        const hash = pcmHash(512, 2, (frame, channel) => pcm.readUInt32LE((first + frame) * 8 + channel * 4));
        let zero = 0;
        for (let frame = first; frame < first + 512; frame++) zero += Number(pcm.readFloatLE(frame * 8) === 0 && pcm.readFloatLE(frame * 8 + 4) === 0);
        rows.push([rows.length + 1, render, 100000 + first, 512, 1, 512,
          zero, hash, render ? 3 : 0, (render ? 10 : 12) + first, (render ? 11 : 13) + first]);
      }
    }
    writeFileSync(args.at(-1), JSON.stringify({ version: 5, capacity: 8192, issued: rows.length, dropped: 0, rows,
      admission: { read: rows.filter(row => row[1] === 0).length, write: rows.filter(row => row[1] === 1).length,
        timestamp: 0, extent: 0, stream: 0, frozen: 0, inactive: 0, native: 0,
        lastRejectedTimestampBits: '0000000000000000', lastRejectedExtent: { frames: 0, bufferPresent: false, render: false },
        nullBuffer: 0, oversizedFrames: 0 } }));
  };
  await assert.rejects(compareMacosSources({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.test'] }, join(directory, 'evidence'), {
    execute, prepare: () => bundle, measurementBoundary: async (measure, options) => { receipts.push(options.receiptDirectory); return measure(); },
  }), /source comparison failed/);
  assert.deepEqual(commands, sourceOrders.map(order => ({ order, bundle })));
  assert.equal(new Set(receipts).size, 2);
  const report = JSON.parse(readFileSync(join(directory, 'evidence/comparison.json')));
  assert.deepEqual(report.results.map(result => result.passed), [false, true]);
  assert.equal(report.qualificationReplacement, false);
  assert.equal(report.passed, false);
});

if (process.platform === 'darwin') test('source IO callback plays silence and signal without reopening; self-test never opens a device', t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-source-native-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const bundle = prepareSourceComparison(directory);
  execFileSync(join(bundle, 'Contents/MacOS/MRSourceControl'), ['--self-test'], { timeout: 5000 });
});
