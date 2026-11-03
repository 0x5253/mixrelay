import assert from 'node:assert/strict';
import test from 'node:test';
import { analyseHalPcmContract } from './hal-pcm-contract.mjs';
import { pcmHash } from './pcm-hash.mjs';

const admission = () => ({ read: 0, write: 0, timestamp: 0, extent: 0, stream: 0, frozen: 0, inactive: 0, native: 0, nullBuffer: 0, oversizedFrames: 0, lastRejectedTimestampBits: '0000000000000000', lastRejectedExtent: { frames: 0, bufferPresent: false, render: false } });
const hash = bytes => pcmHash(bytes.length / 8, 2, (frame, channel) => bytes.readUInt32LE(frame * 8 + channel * 4));
function fixture() {
  const source = Buffer.alloc(12 * 8);
  for (let frame = 0; frame < 8; frame++) {
    source.writeFloatLE(0.125 + frame / 32, frame * 8);
    source.writeFloatLE(0.5 + frame / 32, frame * 8 + 4);
  }
  const returned = Buffer.from(source); returned.fill(0, 4 * 8, 8 * 8);
  const rows = [
    [1, 1, 1000, 4, 1, 4, 0, hash(source.subarray(0, 4 * 8)), 17, 10, 11],
    [2, 1, 1008, 4, 1, 4, 4, hash(source.subarray(8 * 8)), 17, 12, 13],
    [3, 0, 1000, 12, 1, 8, 8, hash(returned), 0, 20, 21],
  ];
  return { source, origin: 1000, channels: 2, sourceQuantum: 4,
    trace: { version: 5, issued: 3, dropped: 0, capacity: 8192, admission: admission(), rows } };
}
const analyse = ({ trace, ...reference }) => analyseHalPcmContract(trace, reference);

test('the HAL preserves delivered signal and silence and zero-fills a source callback gap', () => {
  const result = analyse(fixture());
  assert.equal(result.passed, true);
  assert.equal(result.unpublishedFrames, 4);
  assert.equal(result.comparedFrames, 12);
  assert.deepEqual(result.sourceRanges, [{ start: 0, end: 4 }, { start: 8, end: 12 }]);
});

test('dropping received PCM fails even when all missing frames are returned as zero', () => {
  const data = fixture(), read = data.trace.rows[2];
  read[5] = 4; read[6] = 12; read[7] = hash(Buffer.alloc(12 * 8));
  const result = analyse(data);
  assert.equal(result.passed, false);
  assert.equal(result.violations[0].boundary, 'hal_read');
  assert.equal(result.violations[0].expected.available, 8);
});

test('old audible PCM during published silence or an unpublished gap fails exact sample comparison', () => {
  for (const first of [4, 8]) {
    const data = fixture(), replay = Buffer.from(data.source); replay.fill(0, 4 * 8);
    data.source.copy(replay, first * 8, 0, 4 * 8);
    const read = data.trace.rows[2]; read[7] = hash(replay); read[6] = 4;
    assert.equal(analyse(data).passed, false);
  }
});

test('corrupt publication is not selected away by searching for a matching source hash', () => {
  const data = fixture(); data.trace.rows[0][7] = '0000000000000000';
  const result = analyse(data);
  assert.equal(result.sourceClient, 17);
  assert.equal(result.passed, false);
  assert.equal(result.violations[0].reason, 'changed_source_pcm');
});

test('missing trace rows, native rejection and an overlapping publication cannot produce acceptance', () => {
  const missing = fixture(); missing.trace.rows.pop();
  assert.equal(analyse(missing).passed, false);
  const rejected = fixture(); rejected.trace.rows[0][4] = 0; rejected.trace.rows[0][5] = 0;
  assert.equal(analyse(rejected).passed, false);
  const concurrent = fixture(); concurrent.trace.rows[0][10] = 21;
  const result = analyse(concurrent);
  assert.equal(result.passed, false);
  assert.equal(result.evidenceComplete, false);
  assert.equal(result.evidenceIssues[0].reason, 'concurrent_publication');
});

test('a native inactive rejection remains a product failure despite matching retained PCM', () => {
  const data = fixture();
  data.trace.admission = { ...admission(), read: 1, write: 2, timestamp: 0, extent: 0, stream: 0,
    frozen: 0, inactive: 1, native: 0, lastRejectedTimestampBits: '0000000000000000' };
  assert.equal(analyse(data).passed, false);
});

test('v5 distinguishes rejected invalid host storage from rejection of a valid coalesced IO span', () => {
  const data = fixture();
  data.trace.admission = { ...admission(), read: 2, write: 2, timestamp: 0, extent: 1, stream: 0,
    frozen: 0, inactive: 0, native: 0, nullBuffer: 1, oversizedFrames: 0,
    lastRejectedTimestampBits: '0000000000000000',
    lastRejectedExtent: { frames: 8192, bufferPresent: false, render: false } };
  let measured = analyse(data);
  assert.equal(measured.passed, true);
  assert.equal(measured.rejectedHostRequests.length, 1);
  data.trace.admission.lastRejectedExtent.bufferPresent = true;
  assert.equal(analyse(data).passed, false, 'An 8,192-frame request with valid storage must be accepted');
  data.trace.admission.lastRejectedExtent.frames = 1048577;
  data.trace.admission.nullBuffer = 0;
  data.trace.admission.oversizedFrames = 1;
  assert.equal(analyse(data).passed, true, 'A request beyond prepared retention must be rejected safely');
  data.trace.admission.extent = 2;
  assert.equal(analyse(data).passed, false, 'Every rejection must have a matching typed classification');
  data.trace.admission.oversizedFrames = 2;
  assert.equal(analyse(data).passed, true);
  data.trace.admission.extent = 0;
  assert.equal(analyse(data).passed, false, 'An in-flight classification cannot certify complete admission evidence');
  delete data.trace.admission.lastRejectedExtent;
  assert.throws(() => analyse(data), /rejected extent/);
});

test('old trace schemas cannot establish native PCM evidence', () => {
  for (const version of [1, 2, 3, 4]) {
    const data = fixture(); data.trace.version = version;
    assert.throws(() => analyse(data), /Unsupported HAL trace version/);
  }
});
