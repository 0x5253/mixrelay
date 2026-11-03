import assert from 'node:assert/strict';
import { recordingPcm } from './recording-format.mjs';
import { decodeRecordingCodewords } from './recording-codewords.mjs';
import { intersectPcmRanges, mergePcmRanges, pcmRangeFrames, subtractPcmRanges } from './pcm-ranges.mjs';

function quietAtOffset(data, segments, offset) {
  let comparedFrames = 0, nonzeroFrames = 0;
  const unavailable = [];
  for (const segment of segments.filter(segment => segment.kind === 'silent')) {
    const start = offset + segment.start, end = offset + segment.end;
    if (start < 0 || end > data.length / 8) { unavailable.push(segment); continue; }
    for (let frame = start; frame < end; frame++) {
      comparedFrames++;
      nonzeroFrames += Number(data.readFloatLE(frame * 8) !== 0 || data.readFloatLE(frame * 8 + 4) !== 0);
    }
  }
  return { comparedFrames, nonzeroFrames, unavailable };
}

function inspectOrder(reference, runs, data) {
  const segment = reference.segments.find(segment => segment.kind === 'audible');
  assert.ok(segment, 'Recording reference requires a coded audible segment');
  const expected = intersectPcmRanges(reference.sourceRanges, segment.start, segment.end);
  const observed = runs.map(run => ({ start: run.sourceStart, end: run.sourceStart + run.frames }));
  const missing = subtractPcmRanges(expected, observed), unexpected = subtractPcmRanges(observed, expected);
  const repeatedFrames = pcmRangeFrames(observed) - pcmRangeFrames(mergePcmRanges(observed));
  const offsets = [...new Set(runs.map(run => run.recordingStart - run.sourceStart))];
  const quiet = offsets.length > 0 ? quietAtOffset(data, reference.segments, offsets[0]) : null;
  const expectedFrames = pcmRangeFrames(expected);
  return { order: reference.order, expectedFrames, sourceUnavailableFrames: segment.end - segment.start - expectedFrames,
    missingFrames: pcmRangeFrames(missing), missing, repeatedFrames, unexpected,
    timelineOffsets: offsets, quiet,
    passed: reference.productPassed === true && expectedFrames >= 24000 && runs.length > 0
      && missing.length === 0 && unexpected.length === 0 && repeatedFrames === 0 && offsets.length === 1
      && quiet.nonzeroFrames === 0 && quiet.unavailable.length === 0 };
}

// Expected IDs come from actual native source publications, never from a count
// subtracted from the planned signal. One recording offset must preserve their
// sample coordinates, including zero-filled source gaps and the quiet phases.
export function analyseRecordingPcmContract(bytes, references) {
  assert.ok(Array.isArray(references) && references.length > 0, 'Missing recording input publication evidence');
  assert.ok(references.every(reference => reference && Array.isArray(reference.segments)
    && Array.isArray(reference.sourceRanges) && Number.isSafeInteger(reference.sourceFrames)), 'Incomplete recording input publication evidence');
  const { data, format } = recordingPcm(bytes);
  assert.equal(format.channels, 2);
  assert.equal(format.rate, 48000);
  const decoded = decodeRecordingCodewords(data, Math.max(...references.map(reference => reference.sourceFrames)));
  const ordered = decoded.runs.map(run => {
    const index = references.findIndex(reference => reference.segments.some(segment => segment.kind === 'audible'
      && run.sourceStart >= segment.start && run.sourceStart + run.frames <= segment.end));
    return { ...run, index };
  });
  let lastIndex = -1, orderRegressions = 0;
  for (const run of ordered) {
    if (run.index >= 0) { orderRegressions += Number(run.index < lastIndex); lastIndex = Math.max(lastIndex, run.index); }
  }
  const orders = references.map((reference, index) => inspectOrder(reference, ordered.filter(run => run.index === index), data));
  const unexpectedFrames = ordered.filter(run => run.index < 0).reduce((total, run) => total + run.frames, 0);
  return { boundary: 'engine_recording_pcm', format, orders, unexpectedFrames, orderRegressions,
    inputEvidenceComplete: references.every(reference => reference.productPassed === true),
    nonfiniteSamples: decoded.nonfiniteSamples, unidentifiedNonzeroFrames: decoded.unidentifiedNonzeroFrames,
    passed: orders.every(order => order.passed) && unexpectedFrames === 0 && orderRegressions === 0
      && decoded.nonfiniteSamples === 0 && decoded.unidentifiedNonzeroFrames === 0,
    limitations: ['Silence is checked as zero payload at the audible signal alignment; individual zero samples have no unique identity.',
      'The reference requires complete native publication evidence and cannot assign the cause of missing OS callbacks.'] };
}
