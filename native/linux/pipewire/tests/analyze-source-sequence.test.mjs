import test from 'node:test';
import assert from 'node:assert/strict';
import { sequenceEvents, compareEvents } from './analyze-source-sequence.mjs';

function stereo(tags) {
  const bytes = Buffer.alloc(tags.length * 8);
  tags.forEach((tag, index) => bytes.writeUInt32LE(tag === null ? 0 : 0x3d000000 | tag, index * 8 + 4));
  return bytes;
}

test('exact missing-period, duplicate, invalid, wrap and trim evidence', () => {
  assert.deepEqual(sequenceEvents(stereo([10, 11, 12, 13])), []);
  assert.deepEqual(sequenceEvents(stereo([10, 11, 60])), [
    { kind: 'jump', sampleIndex: 2, expected: 12, actual: 60, signedTagStep: 48 },
  ]);
  assert.deepEqual(sequenceEvents(stereo([10, 10, 11])), [
    { kind: 'jump', sampleIndex: 1, expected: 11, actual: 10, signedTagStep: -1 },
  ]);
  assert.deepEqual(sequenceEvents(stereo([10, null, null, 11])), [
    { kind: 'invalid', sampleIndex: 1, expected: 11, length: 2, resumedAt: 11 },
  ]);
  assert.equal(sequenceEvents(stereo([10, null, 12])).length, 2);
  assert.deepEqual(sequenceEvents(stereo([0x7ffffe, 0x7fffff, 0, 1])), []);
  assert.deepEqual(sequenceEvents(stereo([null, 10, 11, null]), 1, 1), []);
  assert.throws(() => sequenceEvents(Buffer.alloc(7)));
  assert.throws(() => sequenceEvents(stereo([1, 2]), 1, 2));
  assert.throws(() => sequenceEvents(stereo([1, 2]), -1));
});

test('compares source frame identity rather than capture timing or sample offsets', () => {
  const native = sequenceEvents(stereo([10, 11, 60, 61, 100]));
  const independent = sequenceEvents(stereo([9, 10, 11, 60, 61, 62, 110]));
  const compared = compareEvents(native, independent);
  assert.equal(compared.common.length, 1);
  assert.equal(compared.common[0].native.sampleIndex, 2);
  assert.equal(compared.common[0].independent.sampleIndex, 3);
  assert.equal(compared.nativeOnly.length, 1);
  assert.equal(compared.independentOnly.length, 1);
  const repeated = compareEvents([native[0], native[0]], [independent[0]]);
  assert.equal(repeated.common.length, 1);
  assert.equal(repeated.nativeOnly.length, 1);
});
