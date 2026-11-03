import assert from 'node:assert/strict';
import { analyseHalIo } from './analyse-hal-io.mjs';
import { pcmHash } from './pcm-hash.mjs';
import { mergePcmRanges } from './pcm-ranges.mjs';

const overlaps = (a, b) => a[2] < b[2] + b[3] && b[2] < a[2] + a[3];
const supplied = row => row[4] === 1 && row[5] === row[3];

function inspectAdmission(admission, violations, rejectedHostRequests, evidenceIssues) {
  const countsComplete = admission.nullBuffer + admission.oversizedFrames === admission.extent;
  if (!countsComplete) evidenceIssues.push({ reason: 'incomplete_extent_classification', admission });
  if (admission.extent && countsComplete) {
    const extent = admission.lastRejectedExtent;
    const classified = (!extent.bufferPresent ? admission.nullBuffer > 0
        : extent.frames > 1048576 && admission.oversizedFrames > 0);
    if (classified) rejectedHostRequests.push({ reason: 'invalid_buffer_extent', count: admission.extent,
      nullBuffer: admission.nullBuffer, oversizedFrames: admission.oversizedFrames, lastRejectedExtent: extent });
    else violations.push({ boundary: 'hal_admission', reason: 'extent', count: admission.extent,
      lastRejectedExtent: extent });
  }
  for (const stage of ['timestamp', 'stream', 'frozen', 'inactive', 'native'])
    if (admission[stage]) violations.push({ boundary: 'hal_admission', reason: stage, count: admission[stage] });
}

function referenceBits(source, origin, channels) {
  return (deviceFrame, channel) => {
    const frame = deviceFrame - origin;
    return frame < 0 || frame >= source.length / (channels * 4) ? 0
      : source.readUInt32LE((frame * channels + channel) * 4);
  };
}

function inspectWrites(writes, sourceClient, channels, bits, violations) {
  for (const row of writes) {
    if (!supplied(row)) violations.push({ boundary: 'hal_write', sequence: row[0], reason: 'rejected_publication' });
    const sample = (frame, channel) => row[8] === sourceClient ? bits(row[2] + frame, channel) : 0;
    const expected = pcmHash(row[3], channels, sample);
    if (row[7] !== expected) violations.push({ boundary: 'hal_write', sequence: row[0],
      reason: row[8] === sourceClient ? 'changed_source_pcm' : 'unresolved_additional_writer', expected, observed: row[7] });
  }
}

function inspectRead(read, writes, sourceClient, channels, bits) {
  const available = new Uint8Array(read[3]), signal = new Uint8Array(read[3]);
  const concurrentWrites = [], sourceRanges = [];
  for (const write of writes.filter(write => supplied(write) && overlaps(write, read))) {
    if (write[9] < read[10] && write[10] > read[9]) concurrentWrites.push(write[0]);
    if (write[10] > read[9]) continue;
    const first = Math.max(0, write[2] - read[2]), last = Math.min(read[3], write[2] + write[3] - read[2]);
    available.fill(1, first, last);
    if (write[8] === sourceClient) {
      signal.fill(1, first, last);
      sourceRanges.push({ start: read[2] + first, end: read[2] + last });
    }
  }
  let zeroFrames = 0;
  const sample = (frame, channel) => signal[frame] ? bits(read[2] + frame, channel) : 0;
  for (let frame = 0; frame < read[3]; frame++) {
    let zero = true;
    for (let channel = 0; channel < channels; channel++) zero &&= (sample(frame, channel) & 0x7fffffff) === 0;
    zeroFrames += Number(zero);
  }
  return { available: available.reduce((sum, value) => sum + value, 0), zeroFrames,
    hash: pcmHash(read[3], channels, sample), concurrentWrites, sourceRanges };
}

// The oracle starts at observed HAL writes. It never invents publication for a
// missing OS callback, and never accepts missing or ambiguous observer evidence.
export function analyseHalPcmContract(trace, { source, origin, channels, sourceQuantum = 512 }) {
  assert.ok(Number.isSafeInteger(channels) && channels > 0 && channels <= 64);
  assert.equal(source.length % (channels * 4), 0);
  assert.ok(Number.isSafeInteger(origin) && origin >= 0);
  const summary = analyseHalIo(trace), { retention, admission } = summary;
  const writes = trace.rows.filter(row => row[1] === 1), reads = trace.rows.filter(row => row[1] === 0);
  // The installed finite probes fit inside the product's prepared 1,048,576-frame retention.
  assert.ok(trace.rows.every(row => row[3] > 0 && row[3] <= 32768), 'Unsupported native transfer extent');
  const extent = Math.max(...trace.rows.map(row => row[2] + row[3])) - Math.min(...trace.rows.map(row => row[2]));
  assert.ok(extent <= 1048576, 'Measurement exceeds the retained native timeline');
  const clients = [...new Set(writes.map(row => row[8]))];
  const candidates = clients.filter(client => writes.filter(row => row[8] === client).every(row => row[3] === sourceQuantum));
  const sourceClient = candidates.length === 1 ? candidates[0] : null;
  const evidenceIssues = [], violations = [], rejectedHostRequests = [];
  if (sourceClient === null) evidenceIssues.push({ reason: 'unresolved_source_client', candidates });
  if (!writes.length || !reads.length) evidenceIssues.push({ reason: 'missing_native_io' });
  if (!retention.issued || retention.writerDropped || retention.notRetainedRecentRows || retention.observationsBeyondCapacity)
    evidenceIssues.push({ reason: 'incomplete_native_trace', retention });
  inspectAdmission(admission, violations, rejectedHostRequests, evidenceIssues);
  const bits = referenceBits(source, origin, channels);
  if (sourceClient !== null) inspectWrites(writes, sourceClient, channels, bits, violations);
  let comparedFrames = 0, unpublishedFrames = 0;
  const comparedSource = [];
  for (const row of sourceClient === null ? [] : reads) {
    const expected = inspectRead(row, writes, sourceClient, channels, bits);
    if (expected.concurrentWrites.length) {
      evidenceIssues.push({ reason: 'concurrent_publication', sequence: row[0], writes: expected.concurrentWrites });
      continue;
    }
    comparedFrames += row[3];
    unpublishedFrames += row[3] - expected.available;
    comparedSource.push(...expected.sourceRanges);
    if (row[4] !== 1 || row[5] !== expected.available || row[6] !== expected.zeroFrames || row[7] !== expected.hash)
      violations.push({ boundary: 'hal_read', sequence: row[0], frame: row[2], frames: row[3],
        expected: { accepted: 1, available: expected.available, zeroFrames: expected.zeroFrames, hash: expected.hash },
        observed: { accepted: row[4], available: row[5], zeroFrames: row[6], hash: row[7] } });
  }
  const sourceRanges = mergePcmRanges(writes.filter(row => row[8] === sourceClient && supplied(row))
    .flatMap(row => row[2] + row[3] <= origin ? [] : [{ start: Math.max(0, row[2] - origin), end: row[2] + row[3] - origin }]));
  return { boundary: 'hal_pcm', sourceClient, sourceRanges, comparedFrames, unpublishedFrames,
    comparedSourceRanges: mergePcmRanges(comparedSource.flatMap(range => range.end <= origin ? []
      : [{ start: Math.max(0, range.start - origin), end: range.end - origin }])),
    evidenceComplete: evidenceIssues.length === 0, evidenceIssues, violations, rejectedHostRequests,
    passed: evidenceIssues.length === 0 && violations.length === 0 && comparedFrames > 0,
    limitations: ['Only complete, nonconcurrent native observations establish this finite single-source contract.',
      'OS callback cadence and downstream OS delivery remain separate diagnostics; this result does not establish their cause.'] };
}
