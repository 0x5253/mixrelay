import assert from 'node:assert/strict';

const PCM_COUNTERS = ['missingFrames', 'repeatedFrames', 'corruptFrames', 'nonfiniteSamples'];

// The original OS counters remain diagnostic data. Product acceptance compares
// observed native publications with the buffers returned by our HAL.
class MacosPcmFailure extends Error {
  constructor(measured, product) {
    const counters = Object.fromEntries(PCM_COUNTERS.map(field => [field, measured[field]]));
    super(`macOS/CoreAudio/hal_pcm: violations=${product.violations.length} evidenceIssues=${product.evidenceIssues.length}; observed OS roundtrip: ${Object.entries(counters).map(([field, count]) => `${field}=${count}`).join(' ')}`);
    this.name = 'MacosPcmFailure';
    this.measurementFailure = Object.freeze({ os: 'macos', backend: 'CoreAudio', boundary: 'hal_pcm',
      violations: product.violations, evidenceIssues: product.evidenceIssues, osRoundtrip: counters });
  }
}

export function verifyMeasurement(measured, product) {
  assert.equal(measured.inputAuthorization?.final, 3, 'Probe recording consent must precede native I/O');
  assert.equal(measured.inputAuthorization?.initial, 3, 'The unchanged signed probe must retain its preflight consent');
  assert.equal(measured.inputAuthorization?.requested, false, 'Finite PCM cannot request consent while observation is paused');
  assert.equal(measured.activityProtection, true, 'Finite measurement must retain its native activity');
  assert.equal(measured.complete, true);
  assert.equal(typeof measured.channelOrder, 'boolean');
  for (const field of PCM_COUNTERS) assert.ok(Number.isSafeInteger(measured[field]) && measured[field] >= 0, field);
  assert.ok(product?.boundary === 'hal_pcm', 'Missing product PCM boundary evidence');
  if (!product.passed) throw new MacosPcmFailure(measured, product);
  assert.ok(measured.receivedFrames >= 24000);
  return { boundary: 'os_roundtrip', passed: measured.channelOrder && PCM_COUNTERS.every(field => measured[field] === 0),
    ...Object.fromEntries(PCM_COUNTERS.map(field => [field, measured[field]])) };
}
