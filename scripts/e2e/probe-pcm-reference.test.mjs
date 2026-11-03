import assert from 'node:assert/strict';
import test from 'node:test';
import { analyseProbeReference } from './probe-pcm-reference.mjs';
import { probeRangeHash, probeSampleBits } from './probe-signal.mjs';

test('integer reference matches a full healthy HAL write hashed by the native probe candidate', () => {
  // Run 40220258479, macOS 14 ARM fresh: device 100996, output origin 54916.
  assert.equal(probeRangeHash(46080, 512, 2), 'af01f59b78217b25');
});

test('a silent sole writer remains a failed reference and absent writers prove no native comparison', () => {
  const metrics = { sameDevice: true, output: { rate: 40, channels: 2 },
    input: { rate: 40, channels: 2 }, signalFrames: 8, recordedFrames: 8 };
  const timing = 'output,0,100,10,8,1,1,2\ninput,0,104,10,8,1,1,2\n';
  const pcm = Buffer.alloc(64);
  const rows = [[1, 1, 104, 8, 1, 8, 8, '0000000000000000', 42],
    [2, 0, 104, 8, 1, 8, 8, '0000000000000000', 0]];
  const silent = analyseProbeReference({ rows }, metrics, timing, pcm);
  assert.equal(silent.nativeWrites.comparedRows, 1);
  assert.equal(silent.nativeWrites.differences[0].sequence, 1);
  assert.equal(silent.nativeCapture.differences[0].sequence, 2);
  const absent = analyseProbeReference({ rows: rows.slice(1) }, metrics, timing, pcm);
  assert.equal(absent.nativeCapture.comparisonStatus, 'no_retained_output');
  assert.equal(absent.nativeCapture.comparedRows, 0);
});

test('original input mismatch retains exact coordinates and distinguishes zero from wrong PCM', () => {
  const pcm = Buffer.alloc(8 * 2 * 4);
  for (let frame = 0; frame < 8; frame++) for (let channel = 0; channel < 2; channel++) {
    pcm.writeUInt32LE(probeSampleBits(frame + 4, channel), (frame * 2 + channel) * 4);
  }
  const metrics = { sameDevice: true, output: { rate: 40, channels: 2 }, input: { rate: 40, channels: 2 }, signalFrames: 8, recordedFrames: 8 };
  const timing = 'output,0,100,10,8,1,1,2\ninput,0,104,10,8,1,1,2\n';
  const write = [1, 1, 104, 8, 1, 8, 0, probeRangeHash(4, 8, 2)];
  const trace = { rows: [write] };
  assert.deepEqual(analyseProbeReference(trace, metrics, timing, pcm).inputDifferences, []);
  pcm.fill(0, 16, 24);
  pcm.writeFloatLE(0.5, 32);
  const result = analyseProbeReference(trace, metrics, timing, pcm);
  assert.deepEqual(result.nativeWrites.differences, []);
  assert.deepEqual(result.inputDifferences.map(({ deviceStart, deviceEnd, zeroFrames, retainedWriteRows }) =>
    [deviceStart, deviceEnd, zeroFrames, retainedWriteRows.length]), [[106, 107, 1, 1], [108, 109, 0, 1]]);
  assert.throws(() => analyseProbeReference(trace, metrics, timing, pcm.subarray(4)));
});

test('a silent second client does not hide a stale input interval or a native capture fault', () => {
  const pcm = Buffer.alloc(8 * 2 * 4);
  for (let frame = 0; frame < 8; frame++) for (let channel = 0; channel < 2; channel++) {
    pcm.writeUInt32LE(probeSampleBits(frame + 20, channel), (frame * 2 + channel) * 4);
  }
  // Two frames at device 122 repeat one advertised 16-frame period earlier.
  for (let frame = 2; frame < 4; frame++) for (let channel = 0; channel < 2; channel++) {
    pcm.writeUInt32LE(probeSampleBits(frame + 4, channel), (frame * 2 + channel) * 4);
  }
  const metrics = { sameDevice: true, output: { rate: 40, channels: 2 },
    input: { rate: 40, channels: 2, timestampPeriod: 16 }, signalFrames: 32, recordedFrames: 8 };
  const timing = 'output,0,100,10,8,1,1,2\ninput,0,120,10,8,1,1,2\n';
  const rows = [
    [1, 1, 120, 8, 1, 8, 0, probeRangeHash(20, 8, 2), 42],
    [2, 1, 120, 8, 1, 8, 8, '0000000000000000', 43],
    [3, 0, 120, 2, 1, 2, 0, probeRangeHash(20, 2, 2), 0],
    [4, 0, 124, 4, 1, 4, 0, probeRangeHash(24, 4, 2), 0],
  ];
  const result = analyseProbeReference({ rows }, metrics, timing, pcm);
  assert.equal(result.nativeWrites.comparisonStatus, 'signal_plus_silence');
  assert.equal(result.nativeWrites.sourceClient, 42);
  assert.equal(result.nativeCapture.comparedRows, 2);
  assert.deepEqual(result.nativeCapture.differences, []);
  assert.equal(result.inputDifferences[0].matchesPreviousTimestampPeriod, true);
  assert.equal(result.inputDifferences[0].frames, 2);
  assert.deepEqual(result.inputDifferences[0].retainedCaptureRows, []);
  rows[3][7] = 'fedcba0987654321';
  assert.equal(analyseProbeReference({ rows }, metrics, timing, pcm).nativeCapture.differences[0].sequence, 4);
  rows[1][6] = 7;
  const unknownMix = analyseProbeReference({ rows }, metrics, timing, pcm);
  assert.equal(unknownMix.nativeWrites.comparisonStatus, 'inapplicable_multiple_client_mix');
  assert.equal(unknownMix.nativeCapture.comparedRows, 0);
});
