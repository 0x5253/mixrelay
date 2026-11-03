import assert from 'node:assert/strict';
import test from 'node:test';
import { probeTimelineCoverage } from './probe-timeline-coverage.mjs';

test('source callback gaps remain distinct from a complete native input read', () => {
  const rows = [
    { side: 'output', sample: 100, frames: 4 },
    { side: 'output', sample: 108, frames: 4 },
    { side: 'input', sample: 100, frames: 12 },
  ];
  const trace = { rows: [[1, 1, 100, 4, 1, 4, 0, '', 42], [2, 1, 108, 4, 1, 4, 0, '', 42],
    [3, 0, 100, 12, 1, 8, 4, '', 0]] };
  const result = probeTimelineCoverage(rows, trace, { start: 100, end: 112, sourceClient: 42 });
  assert.deepEqual(result.outputCallbacks.missing, [{ start: 104, end: 108 }]);
  assert.deepEqual(result.sourceNativeWrites.missing, result.outputCallbacks.missing);
  assert.equal(result.inputCallbacks.missingFrames, 0);
  assert.equal(result.nativeReads.missingFrames, 0);
});

test('overlaps, rejected writes and another writer cannot supply source coverage', () => {
  const rows = [{ side: 'output', sample: 96, frames: 10 },
    { side: 'output', sample: 104, frames: 10 }];
  const trace = { rows: [[1, 1, 100, 4, 1, 4, 0, '', 42], [2, 1, 104, 4, 0, 0, 0, '', 42],
    [3, 1, 104, 8, 1, 8, 0, '', 43], [4, 0, 102, 8, 1, 8, 0, '', 0]] };
  const result = probeTimelineCoverage(rows, trace, { start: 100, end: 112, sourceClient: 42 });
  assert.equal(result.outputCallbacks.coveredFrames, 12);
  assert.equal(result.outputCallbacks.duplicateFrames, 2);
  assert.deepEqual(result.sourceNativeWrites.missing, [{ start: 104, end: 112 }]);
  assert.deepEqual(result.nativeReads.missing, [{ start: 100, end: 102 }, { start: 110, end: 112 }]);
});

test('an unidentified source remains unknown and empty callback evidence retains the whole gap', () => {
  const result = probeTimelineCoverage([], { rows: [] }, { start: 100, end: 112, sourceClient: null });
  assert.equal(result.sourceNativeWrites, null);
  assert.equal(result.outputCallbacks.missingFrames, 12);
  assert.equal(result.inputCallbacks.missingFrames, 12);
});
