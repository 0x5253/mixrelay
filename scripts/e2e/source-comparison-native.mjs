import assert from 'node:assert/strict';
import { analyseHalIo } from './analyse-hal-io.mjs';

function coverage(rows, start, end) {
  let cursor = start;
  const unobservedRanges = [];
  for (const row of rows.toSorted((a, b) => a[2] - b[2])) {
    const first = Math.max(start, row[2]), last = Math.min(end, row[2] + row[3]);
    if (last <= first) continue;
    if (first > cursor) unobservedRanges.push({ start: cursor, end: first });
    cursor = Math.max(cursor, last);
  }
  if (cursor < end) unobservedRanges.push({ start: cursor, end });
  return { frames: end - start, unobservedFrames: unobservedRanges.reduce((sum, row) => sum + row.end - row.start, 0), unobservedRanges };
}

// Available is a count, not a valid prefix. Only a fully supplied read covers its entire range.
export function analyseSourceNative(trace, plan, source, origin) {
  const summary = analyseHalIo(trace), writes = trace.rows.filter(row => row[1] === 1), reads = trace.rows.filter(row => row[1] === 0);
  assert.equal(source.length, plan.frames * 8);
  assert.ok(Number.isSafeInteger(origin) && origin >= 0);
  const clients = [...new Set(writes.map(row => row[8]))];
  const candidates = clients.filter(client => writes.filter(row => row[8] === client).every(row => row[3] === plan.bufferFrames));
  const sourceClient = candidates.length === 1 ? candidates[0] : null;
  const hashes = new Map();
  const expected = row => {
    const key = `${row[2]}:${row[3]}`;
    if (!hashes.has(key)) {
      let hash = 0xcbf29ce484222325n;
      const start = (row[2] - origin) * 8, end = start + row[3] * 8;
      for (let offset = start; offset < end; offset += 4)
        hash = BigInt.asUintN(64, (hash ^ BigInt(source.readUInt32LE(offset))) * 0x100000001b3n);
      hashes.set(key, hash.toString(16).padStart(16, '0'));
    }
    return hashes.get(key);
  };
  const differences = rows => rows.flatMap(row => row[7] === expected(row) ? [] : [{ sequence: row[0],
    frame: row[2], frames: row[3], observed: row[7], expected: expected(row) }]);
  const retained = summary.retention;
  const admissionComplete = ['timestamp', 'extent', 'stream', 'frozen', 'native']
    .every(stage => summary.admission[stage] === 0);
  const completeRetention = retained.issued > 0 && retained.writerDropped === 0
    && retained.notRetainedRecentRows === 0 && retained.observationsBeyondCapacity === 0;
  const segments = plan.segments.map(segment => {
    const start = origin + segment.start, end = origin + segment.end;
    const overlaps = row => row[2] < end && row[2] + row[3] > start;
    const full = row => row[2] >= start && row[2] + row[3] <= end;
    const supplied = row => row[4] === 1 && row[5] === row[3];
    const sourceWrites = writes.filter(row => row[8] === sourceClient && overlaps(row));
    const otherWrites = writes.filter(row => row[8] !== sourceClient && overlaps(row));
    const knownMix = sourceClient !== null && otherWrites.every(row => supplied(row) && row[6] === row[3]);
    const captures = reads.filter(overlaps);
    const suppliedReads = captures.filter(supplied), deliveredWrites = sourceWrites.filter(supplied);
    const writeCoverage = coverage(deliveredWrites, start, end), observedReadCoverage = coverage(captures, start, end);
    const suppliedReadCoverage = coverage(suppliedReads, start, end);
    const writeDifferences = differences(deliveredWrites.filter(full));
    const lacksSource = row => writeCoverage.unobservedRanges.some(gap => row[2] < gap.end && row[2] + row[3] > gap.start);
    const unpublishedSourceReads = suppliedReads.filter(full).filter(lacksSource);
    const comparedReads = knownMix ? suppliedReads.filter(full).filter(row => !lacksSource(row)) : [];
    const captureDifferences = differences(comparedReads);
    return { ...segment, sourceClient, captureComparison: knownMix ? 'source_plus_silence' : 'unresolved_client_mix',
      writeCoverage, observedReadCoverage, suppliedReadCoverage, writeDifferences, captureDifferences,
      captureComparedRows: comparedReads.length, unpublishedSourceReads,
      rejectedWrites: sourceWrites.filter(row => !supplied(row)), shortOrRejectedReads: captures.filter(row => !supplied(row)),
      nativeEvidenceComplete: admissionComplete && completeRetention && knownMix && writeCoverage.unobservedFrames === 0
        && suppliedReadCoverage.unobservedFrames === 0 && writeDifferences.length === 0 && captureDifferences.length === 0 };
  });
  return { sourceClient, sourceCandidates: candidates, retention: retained, admission: summary.admission, segments,
    nativeEvidenceComplete: segments.every(segment => segment.nativeEvidenceComplete),
    limitations: [...summary.limitations,
      'Source client identity is resolved only when exactly one retained writer has the fixed probe quantum.',
      'Hash comparisons exclude partial segment rows. Coverage still includes their full native ranges.',
      'Capture reference comparison also excludes ranges without retained source publication; missing source coverage remains a failure.',
      'Complete native evidence does not make silent samples individually identifiable or replace qualification.'] };
}
