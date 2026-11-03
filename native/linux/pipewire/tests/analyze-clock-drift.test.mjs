import { test } from 'node:test';
import assert from 'node:assert/strict';
import { checkSinkCapture, checkThreadPolicy, measurePcmWindow, sinkPcmFailures } from './analyze-clock-drift.mjs';

const owners = { pipewirePid: 101, enginePid: 202 };
const policy = (server = 'RR 20', engine = 'RR 20') => `PID TID CLS RTPRIO COMMAND
101 102 ${server} data-loop.0
202 203 ${engine} data-loop.0
999 1000 RR 20 data-loop.0`;

test('RT engine clients cannot qualify a non-RT server clock driver', () => {
  assert.deepEqual(checkThreadPolicy(policy('TS -'), owners).schedulerFailures,
    ['owned_server_data_loop_not_rr20']);
  assert.deepEqual(checkThreadPolicy(policy('RR 19'), owners).schedulerFailures,
    ['owned_server_data_loop_not_rr20']);
  assert.deepEqual(checkThreadPolicy(policy(), owners).schedulerFailures, []);
});

test('all owned data loops require the requested policy and priority', () => {
  assert.deepEqual(checkThreadPolicy(`${policy()}\n202 204 TS - data-loop.1`, owners).schedulerFailures,
    ['owned_engine_data_loop_not_rr20']);
  assert.deepEqual(checkThreadPolicy(policy('RR 20', 'FF 20'), owners).schedulerFailures,
    ['owned_engine_data_loop_not_rr20']);
});

test('unrelated RT threads, missing data loops and absent owners cannot qualify', () => {
  assert.deepEqual(checkThreadPolicy(policy().replace('101 102', '303 304'), owners).schedulerFailures,
    ['owned_server_data_loop_not_rr20']);
  assert.deepEqual(checkThreadPolicy(policy().replace('data-loop.0', 'module-rt'), owners).schedulerFailures,
    ['owned_server_data_loop_not_rr20']);
  assert.throws(() => checkThreadPolicy(policy(), {}), /Missing owned process/);
  assert.throws(() => checkThreadPolicy(policy(), { pipewirePid: 101, enginePid: 101 }), /distinct/);
});

test('sink capture requires matching raw bytes and actual bounded callback counts', () => {
  const capture = { sampleRate: 48000, channels: 2, capacityFrames: 256,
    capturedFrames: 128, observedFrames: 128, callbacks: 1,
    invalidCallbacks: 0, rawBytes: 1024, capHit: false };
  assert.equal(checkSinkCapture({ sinkPcmCapture: capture }, 1024), capture);
  assert.throws(() => checkSinkCapture({}, 1024), /Missing native/);
  for (const patch of [{ capturedFrames: 257 }, { observedFrames: 127 },
    { callbacks: 0 }, { invalidCallbacks: 1 }, { rawBytes: 1025 },
    { channels: 1 }, { capHit: true }, { observedFrames: NaN }]) {
    assert.throws(() => checkSinkCapture({ sinkPcmCapture: { ...capture, ...patch } }, 1024));
  }
  assert.throws(() => checkSinkCapture({ sinkPcmCapture: capture }, 1023), /incomplete/);
  const capped = { ...capture, capturedFrames: 256, observedFrames: 300,
    rawBytes: 2048, callbacks: 3, capHit: true };
  assert.equal(checkSinkCapture({ sinkPcmCapture: capped }, 2048), capped);
});

test('actual PCM measurement rejects a missing 128-frame callback block', () => {
  const frames = 4800;
  const bytes = Buffer.alloc(frames * 8);
  for (let frame = 0; frame < frames; frame++) {
    const value = 0.25 * Math.sin(2 * Math.PI * 1000 * frame / 48000);
    bytes.writeFloatLE(value, frame * 8);
    bytes.writeFloatLE(value, frame * 8 + 4);
  }
  const capture = { missingTimestamps: 0, cycleDiscontinuities: 0,
    droppedCycleObservations: 0, cycleEvents: [] };
  const measured = () => ({ ...measurePcmWindow(bytes, 0, frames),
    measuredStartFrame: 0, capture });
  const clean = measured();
  assert.equal(clean.invalid, 0);
  assert.equal(clean.stereoMismatches, 0);
  assert.equal(clean.maximumGap, 1); // Valid sine zero crossings, not lost blocks.
  assert.deepEqual(sinkPcmFailures(clean), []);
  bytes.fill(0, 997 * 8, (997 + 128) * 8);
  const dropped = measured();
  assert.equal(dropped.maximumGap, 128);
  assert.equal(dropped.jumps, 2);
  assert.deepEqual(sinkPcmFailures(dropped), ['sink_pcm_discontinuity_or_corruption']);
});

test('clean PCM cannot conceal a missing native cycle inside the measured window', () => {
  const pcm = { measuredStartFrame: 960000, frames: 1776000, peak: 0.25,
    capture: { missingTimestamps: 0, cycleDiscontinuities: 1,
      droppedCycleObservations: 0, cycleEvents: [{ frame: 1713280 }] } };
  assert.deepEqual(sinkPcmFailures(pcm), ['sink_native_cycle_discontinuity']);
  pcm.capture.cycleEvents[0].frame = pcm.measuredStartFrame - 1;
  assert.deepEqual(sinkPcmFailures(pcm), []);
  pcm.capture.cycleEvents[0].frame = pcm.measuredStartFrame + pcm.frames;
  assert.deepEqual(sinkPcmFailures(pcm), []);
  pcm.capture.droppedCycleObservations = 1;
  assert.deepEqual(sinkPcmFailures(pcm), ['sink_native_timeline_incomplete']);
  pcm.capture.droppedCycleObservations = 0;
  pcm.capture.missingTimestamps = 1;
  assert.deepEqual(sinkPcmFailures(pcm), ['sink_native_timeline_incomplete']);
});
