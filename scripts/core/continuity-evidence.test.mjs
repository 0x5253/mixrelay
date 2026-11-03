import assert from 'node:assert/strict';
import test from 'node:test';
import { continuityFailures, parseNativeTelemetry, referenceFailures } from './continuity-evidence.mjs';

function observation() {
  return {
    at: '2026-11-02T00:00:01.000Z',
    status: { underruns: 0, overruns: 0, audioSession: { discontinuities: 0, format: { sampleRate: 48000 }, processingState: 'running' } },
    devices: { input: { at: '2026-11-02T00:00:00.000Z', pcm_starvations: 0, epoch: 1, native_rate: 48000, enabled: true, ready: true } },
  };
}

test('continuous observations reject counter changes, stale telemetry and unhealthy sessions', () => {
  const before = observation(), last = observation();
  const now = Date.parse(last.at);
  assert.deepEqual(continuityFailures(before, last, now), []);
  last.status.underruns++;
  last.status.audioSession.processingState = 'stopped';
  last.devices.input.epoch++;
  const failures = continuityFailures(before, last, now);
  assert.equal(failures.length, 3);
  assert.ok(failures.some((failure) => failure.includes('underruns 0 -> 1')));
  assert.ok(failures.some((failure) => failure.includes('epoch 1 -> 2')));
  assert.match(continuityFailures(before, observation(), now + 5000)[0], /missing\/stale/);
  const invalid = observation();
  invalid.devices.input.at = 'invalid';
  assert.match(continuityFailures(before, invalid, now)[0], /missing\/stale/);
});

test('reference comparisons retain previous errors even when the current observation window is steady', () => {
  const reference = { type: 'result', after: observation(), afterGraphs: [{ revision: 1 }] };
  const last = observation();
  last.status.underruns = 2;
  assert.deepEqual(continuityFailures(last, structuredClone(last), Date.parse(last.at)), []);
  assert.match(referenceFailures(reference, last, reference.afterGraphs)[0], /Reference underruns: 0 -> 2/);
  assert.throws(() => referenceFailures({ type: 'start' }, last, []), /completed observation/);
});

test('native telemetry parser ignores incomplete boundary lines and preserves per-owner queue and timing observations', () => {
  const parsed = parseNativeTelemetry('partial\n2026-11-02T00:00:00Z native PCM queues owner=input ready=true epoch=3\n2026-11-02T00:00:00Z PCM callback execution timing owner=input maximum=42\nincomplete');
  assert.equal(parsed.devices.input.ready, true);
  assert.equal(parsed.devices.input.epoch, 3);
  assert.equal(parsed.timing.input.maximum, 42);
});
