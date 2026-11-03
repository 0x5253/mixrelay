import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { decodeRecordingCodewords } from './recording-codewords.mjs';
import { analyseSourceRecording } from './source-recording-analysis.mjs';
import { recordSourceComparison } from './source-comparison-recording.mjs';
import { sourceComparisonPlan, sourceComparisonPcm } from './source-comparison-plan.mjs';
import { analyseRecordingPcmContract } from './recording-pcm-contract.mjs';

function recording(pcm) {
  const header = Buffer.alloc(80);
  header.write('RF64'); header.writeUInt32LE(0xffffffff, 4); header.write('WAVE', 8);
  header.write('ds64', 12); header.writeUInt32LE(28, 16);
  header.writeBigUInt64LE(BigInt(80 + pcm.length - 8), 20);
  header.writeBigUInt64LE(BigInt(pcm.length), 28); header.writeBigUInt64LE(BigInt(pcm.length / 8), 36);
  header.write('fmt ', 48); header.writeUInt32LE(16, 52); header.writeUInt16LE(3, 56);
  header.writeUInt16LE(2, 58); header.writeUInt32LE(48000, 60); header.writeUInt32LE(384000, 64);
  header.writeUInt16LE(8, 68); header.writeUInt16LE(32, 70);
  header.write('data', 72); header.writeUInt32LE(0xffffffff, 76);
  return Buffer.concat([header, pcm]);
}
const first = () => sourceComparisonPcm(sourceComparisonPlan('silent-audible'));
const second = () => sourceComparisonPcm(sourceComparisonPlan('audible-silent'));
const take = (a = first(), b = second()) => recording(Buffer.concat([Buffer.alloc(337 * 8), a, Buffer.alloc(2003 * 8), b]));
const references = () => ['silent-audible', 'audible-silent'].map(order => {
  const plan = sourceComparisonPlan(order);
  return { ...plan, sourceFrames: plan.frames, productPassed: true, sourceRanges: [{ start: 0, end: plan.frames }] };
});

test('recording acceptance requires every published frame while retaining an unproduced source gap as zero', () => {
  const source = first(), start = 241152, end = 242148, refs = references();
  source.fill(0, start * 8, end * 8);
  refs[0].sourceRanges = [{ start: 0, end: start }, { start: end, end: refs[0].sourceFrames }];
  const bytes = take(source), result = analyseRecordingPcmContract(bytes, refs);
  assert.equal(analyseSourceRecording(bytes).passed, false, 'The original planned-signal diagnosis is retained');
  assert.equal(result.passed, true);
  assert.equal(result.orders[0].sourceUnavailableFrames, 996);
  assert.equal(result.orders[0].missingFrames, 0);
  assert.equal(result.orders[0].quiet.comparedFrames, 144000);
});

test('a same-sized source gap elsewhere cannot excuse lost delivered PCM', () => {
  const source = first(), refs = references(); source.fill(0, 241152 * 8, 242148 * 8);
  refs[0].sourceRanges = [{ start: 0, end: 200000 }, { start: 200996, end: refs[0].sourceFrames }];
  const result = analyseRecordingPcmContract(take(source), refs);
  assert.equal(result.passed, false);
  assert.deepEqual(result.orders[0].missing, [{ start: 241152, end: 242148 }]);
  assert.deepEqual(result.orders[0].unexpected, [{ start: 200000, end: 200996 }]);
});

test('one additional lost sample, an inserted quiet block or an old audible replay still fails the product contract', () => {
  const refs = references(), source = first(), start = 241152, end = 242148;
  refs[0].sourceRanges = [{ start: 0, end: start }, { start: end, end: refs[0].sourceFrames }];
  source.fill(0, start * 8, end * 8);
  const lost = Buffer.from(source); lost.fill(0, 250000 * 8, 250001 * 8);
  assert.equal(analyseRecordingPcmContract(take(lost), refs).passed, false);
  const inserted = Buffer.concat([source.subarray(0, 260000 * 8), Buffer.alloc(128 * 8), source.subarray(260000 * 8)]);
  assert.equal(analyseRecordingPcmContract(take(inserted), refs).passed, false);
  const replay = second(); replay.copy(replay, 153200 * 8, 136816 * 8, 136872 * 8);
  const measured = analyseRecordingPcmContract(take(source, replay), refs);
  assert.equal(measured.passed, false);
  assert.equal(measured.orders[1].quiet.nonzeroFrames, 56);
});

test('incomplete publication proof and an absent input cannot qualify a complete-looking recording', () => {
  const refs = references(); refs[0].productPassed = false;
  assert.equal(analyseRecordingPcmContract(take(), refs).passed, false);
  refs[0].productPassed = true; refs[0].sourceRanges = [];
  assert.equal(analyseRecordingPcmContract(take(), refs).passed, false);
  assert.throws(() => analyseRecordingPcmContract(take(), []), /Missing recording input publication evidence/);
});

test('the finalized take identifies every frame in both source orders at arbitrary recording offsets', () => {
  const measured = analyseSourceRecording(take());
  assert.equal(measured.passed, true);
  assert.deepEqual(measured.orders.map(order => [order.order, order.missingFrames, order.repeatedFrames]),
    [['silent-audible', 0, 0], ['audible-silent', 0, 0]]);
  assert.equal(measured.identifiedFrames, 288000);
  assert.equal(measured.quietSampleIdentityObservable, false);
  // Boundaries between codewords must not manufacture one to three lost samples.
  const source = first().subarray(148801 * 8, 148817 * 8);
  assert.deepEqual(decodeRecordingCodewords(source, 364800).runs,
    [{ recordingStart: 0, sourceStart: 148801, frames: 16 }]);
});

test('the observed 524 lost source frames and inserted 128 quiet frames fail a take with normal peaks', () => {
  const source = first(), start = 281492, end = 282016;
  const broken = Buffer.concat([source.subarray(0, start * 8), Buffer.alloc(128 * 8), source.subarray(end * 8)]);
  const measured = analyseSourceRecording(take(broken));
  assert.equal(measured.passed, false);
  assert.equal(measured.unidentifiedNonzeroFrames, 0);
  assert.equal(measured.orders[0].missingFrames, 524);
  assert.deepEqual(measured.orders[0].missing, [{ start, end }]);
  assert.deepEqual(measured.orders[0].recordingGaps, [{ start: 337 + start, end: 337 + start + 128 }]);
});

test('a previous-period audible replay during quiet input fails even when all intended audible frames exist', () => {
  const source = second(), quiet = 153200;
  source.copy(source, quiet * 8, (quiet - 16384) * 8, (quiet - 16384 + 56) * 8);
  const measured = analyseSourceRecording(take(first(), source));
  assert.equal(measured.passed, false);
  assert.equal(measured.orders[1].missingFrames, 0);
  assert.equal(measured.orders[1].repeatedFrames, 56);
});

test('inserted silence, source reordering, swapped channels and nonfinite PCM cannot pass complete set coverage', () => {
  const source = first(), split = 200000;
  const cases = [
    Buffer.concat([source.subarray(0, split * 8), Buffer.alloc(128 * 8), source.subarray(split * 8)]),
    Buffer.concat([source.subarray(0, 148800 * 8), source.subarray(split * 8, 292800 * 8), source.subarray(148800 * 8, split * 8), source.subarray(292800 * 8)]),
  ];
  for (const broken of cases) assert.equal(analyseSourceRecording(take(broken)).passed, false);
  const corrupt = first(); corrupt.writeUInt32LE(corrupt.readUInt32LE(split * 8), split * 8 + 4);
  assert.equal(analyseSourceRecording(take(corrupt)).unidentifiedNonzeroFrames, 1);
  const nonfinite = first(); nonfinite.writeFloatLE(NaN, split * 8 + 4);
  const measured = analyseSourceRecording(take(nonfinite));
  assert.equal(measured.passed, false);
  assert.equal(measured.nonfiniteSamples, 1);
  assert.equal(analyseSourceRecording(take(Buffer.alloc(source.length), Buffer.alloc(source.length))).passed, false);
});

test('recording finalization retains the native failure and the independent recording gap', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-recording-integrity-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = first(), bytes = take(Buffer.concat([source.subarray(0, 200000 * 8), source.subarray(200524 * 8)]));
  let state = 'stopped';
  await assert.rejects(recordSourceComparison({ directory, recorderPath: 'recorder', revision: 7,
    api: async () => ({ state, path: 'take.wav' }),
    controlTake: async value => {
      state = value.request.action === 'record' ? 'recording' : 'stopped';
      if (state === 'stopped') writeFileSync(join(directory, 'recording.wav'), bytes);
      return { state, error: null };
    }, compare: async () => {
      writeFileSync(join(directory, 'comparison.json'), JSON.stringify({ results: references().map(reference => ({ reference })) }));
      throw new Error('native PCM mismatch');
    } }), error => {
    assert.equal(error.errors.length, 2);
    assert.match(error.errors[0].message, /native PCM mismatch/);
    assert.match(error.errors[1].message, /recording lost/);
    return true;
  });
  assert.equal(state, 'stopped');
  const analysis = JSON.parse(readFileSync(join(directory, 'recording-analysis.json')));
  assert.equal(analysis.orders[0].missingFrames, 524);
  assert.equal(analysis.recordingSha256, createHash('sha256').update(bytes).digest('hex'));
});

test('a terminal failed take retains its finalized PCM evidence and cannot pass on complete payload', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-recording-failed-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const bytes = take(), actions = [];
  let state = 'stopped';
  await assert.rejects(recordSourceComparison({ directory, recorderPath: 'recorder', revision: 7,
    api: async () => ({ state, path: join(directory, 'recording.wav'),
      error: state === 'failed' ? 'recorder packet queue overrun' : null }),
    controlTake: async request => {
      actions.push(request.request.action);
      state = 'recording';
      return { state, error: null };
    }, compare: async () => {
      writeFileSync(join(directory, 'recording.wav'), bytes);
      state = 'failed';
      return references().map(reference => ({ reference }));
    } }), AggregateError);
  assert.equal(existsSync(join(directory, 'recording-analysis.json')), true,
    'the recorder already closed its failed take, so its PCM must remain analysable');
  const stopped = JSON.parse(readFileSync(join(directory, 'recording-stop.json')));
  assert.equal(stopped.state, 'failed');
  assert.equal(stopped.error, 'recorder packet queue overrun');
  const analysis = JSON.parse(readFileSync(join(directory, 'recording-analysis.json')));
  assert.equal(analysis.passed, true);
  assert.equal(analysis.recordingSha256, createHash('sha256').update(bytes).digest('hex'));
  assert.deepEqual(actions, ['record']);
});
