import assert from 'node:assert/strict';
import test from 'node:test';
import { analyseSourceNative } from './source-comparison-native.mjs';
import { probeRangeHash, probeSampleBits } from './probe-signal.mjs';

const admission = () => ({ read: 0, write: 0, timestamp: 0, extent: 0, stream: 0, frozen: 0, inactive: 0, native: 0, nullBuffer: 0, oversizedFrames: 0, lastRejectedTimestampBits: '0000000000000000', lastRejectedExtent: { frames: 0, bufferPresent: false, render: false } });
function fixture() {
  const origin = 16384, plan = { frames: 2048, channels: 2, bufferFrames: 512,
    segments: [{ kind: 'silent', start: 0, end: 1024 }, { kind: 'audible', start: 1024, end: 2048 }] };
  const source = Buffer.alloc(2048 * 8), rows = [];
  for (let frame = 1024; frame < 2048; frame++) for (let channel = 0; channel < 2; channel++)
    source.writeUInt32LE(probeSampleBits(frame, channel), frame * 8 + channel * 4);
  const add = (render, first, frames, hash, client) => rows.push([rows.length + 1, render, origin + first, frames, 1, frames,
    first < 1024 || client === 2 ? frames : 0, hash, client, rows.length * 100, rows.length * 100 + 1]);
  for (let first = 0; first < 2048; first += 512) {
    const hash = first < 1024 ? '51d88627df287325' : probeRangeHash(first, 512, 2);
    add(1, first, 512, hash, 1);
    for (let offset = 0; offset < 512; offset += 128) add(1, first + offset, 128, 'd80ac658736bb725', 2);
    add(0, first, 512, hash, 0);
  }
  return { origin, plan, source, trace: { version: 5, issued: rows.length, dropped: 0, capacity: 8192, admission: admission(), rows } };
}
const analyse = ({ trace, plan, source, origin }) => analyseSourceNative(trace, plan, source, origin);

test('native evidence distinguishes fully published silence from the other silent client', () => {
  const result = analyse(fixture());
  assert.equal(result.sourceClient, 1);
  assert.equal(result.nativeEvidenceComplete, true);
  for (const phase of result.segments) {
    assert.equal(phase.writeCoverage.unobservedFrames, 0);
    assert.equal(phase.suppliedReadCoverage.unobservedFrames, 0);
    assert.deepEqual(phase.captureDifferences, []);
  }
});

test('a pre-native rejection remains a failure even when every retained PCM row matches', () => {
  const data = fixture();
  data.trace.admission = { ...admission(), read: 5, write: 20, timestamp: 0, extent: 0, stream: 0,
    frozen: 1, inactive: 0, native: 0, lastRejectedTimestampBits: '0000000000000000' };
  const result = analyse(data);
  assert.equal(result.admission.frozen, 1);
  assert.equal(result.nativeEvidenceComplete, false);
  assert.ok(result.segments.every(segment => segment.captureDifferences.length === 0));
  data.trace.admission.frozen = 0;
  assert.equal(analyse(data).nativeEvidenceComplete, true);
});

test('zero-valued native input with no supplied frames cannot establish valid silence', () => {
  const data = fixture(), read = data.trace.rows.find(row => row[1] === 0);
  read[5] = 0;
  const quiet = analyse(data).segments[0];
  assert.equal(quiet.observedReadCoverage.unobservedFrames, 0);
  assert.equal(quiet.suppliedReadCoverage.unobservedFrames, 512);
  assert.equal(quiet.nativeEvidenceComplete, false);
  assert.equal(quiet.shortOrRejectedReads.length, 1);
});

test('an unobserved quiet native read range remains unresolved despite all-zero payload hashes', () => {
  const data = fixture(), read = data.trace.rows.find(row => row[1] === 0);
  data.trace.rows = data.trace.rows.filter(row => row !== read);
  // The missing trace sequence is an explicit retention limitation as well as a spatial hole.
  const result = analyse(data), quiet = result.segments[0];
  assert.equal(quiet.observedReadCoverage.unobservedFrames, 512);
  assert.deepEqual(quiet.observedReadCoverage.unobservedRanges, [{ start: 16384, end: 16896 }]);
  assert.equal(result.nativeEvidenceComplete, false);
});

test('another silent writer cannot hide rejected publication by the selected source', () => {
  const data = fixture(), write = data.trace.rows.find(row => row[1] === 1 && row[8] === 1);
  write[4] = 0; write[5] = 0;
  const quiet = analyse(data).segments[0];
  assert.equal(quiet.suppliedReadCoverage.unobservedFrames, 0);
  assert.equal(quiet.writeCoverage.unobservedFrames, 512);
  assert.equal(quiet.rejectedWrites.length, 1);
  assert.equal(quiet.nativeEvidenceComplete, false);
});

test('a wrong native capture hash is reported even when availability and zero-frame metadata claim completeness', () => {
  const data = fixture(), read = data.trace.rows.find(row => row[1] === 0);
  read[7] = '0000000000000000';
  const quiet = analyse(data).segments[0];
  assert.equal(quiet.captureDifferences.length, 1);
  assert.equal(quiet.nativeEvidenceComplete, false);
});

test('unknown additional audio is not compared as a source-plus-silence mix', () => {
  const data = fixture(), write = data.trace.rows.find(row => row[8] === 2);
  write[6] = 0;
  const quiet = analyse(data).segments[0];
  assert.equal(quiet.captureComparison, 'unresolved_client_mix');
  assert.equal(quiet.nativeEvidenceComplete, false);
  assert.deepEqual(quiet.captureDifferences, []);
});

test('missing source publication is localized before comparing capture against an unplayed signal', () => {
  const data = fixture();
  data.trace.rows = data.trace.rows.filter(row => !(row[1] === 1 && row[8] === 1 && row[2] === data.origin + 1024));
  data.trace.rows.forEach((row, index) => { row[0] = index + 1; });
  data.trace.issued = data.trace.rows.length;
  const capture = data.trace.rows.find(row => row[1] === 0 && row[2] === data.origin + 1024);
  capture[6] = 512; capture[7] = '51d88627df287325';
  const audible = analyse(data).segments[1];
  assert.equal(audible.writeCoverage.unobservedFrames, 512);
  assert.equal(audible.nativeEvidenceComplete, false);
  assert.deepEqual(audible.captureDifferences, []);
  assert.equal(audible.unpublishedSourceReads.length, 1);
});

test('old trace schemas cannot establish native PCM evidence', () => {
  for (const version of [1, 2, 3, 4]) {
    const data = fixture(); data.trace.version = version;
    assert.throws(() => analyse(data), /Unsupported HAL trace version/);
  }
});
