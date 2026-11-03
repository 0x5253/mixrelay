import { recordingPcm } from './recording-format.mjs';
import { decodeRecordingCodewords } from './recording-codewords.mjs';
import { sourceComparisonPlan, sourceOrders } from './source-comparison-plan.mjs';

function coverage(runs, segment) {
  let cursor = segment.start, observed = 0;
  const missing = [];
  for (const run of runs.toSorted((a, b) => a.sourceStart - b.sourceStart)) {
    const start = Math.max(segment.start, run.sourceStart), end = Math.min(segment.end, run.sourceStart + run.frames);
    if (end <= start) continue;
    observed += end - start;
    if (start > cursor) missing.push({ start: cursor, end: start });
    cursor = Math.max(cursor, end);
  }
  if (cursor < segment.end) missing.push({ start: cursor, end: segment.end });
  const missingFrames = missing.reduce((sum, range) => sum + range.end - range.start, 0);
  return { missingFrames, repeatedFrames: observed - (segment.end - segment.start - missingFrames), missing };
}

// Signal IDs distinguish the two fixed orders without guessing a recording start time.
// Quiet sample identity still requires the independent callback/publication evidence.
export function analyseSourceRecording(bytes) {
  const { data, format } = recordingPcm(bytes);
  const plans = sourceOrders.map(sourceComparisonPlan), decoded = decodeRecordingCodewords(data, plans[0].frames);
  const expected = plans.map(plan => ({ order: plan.order, ...plan.segments.find(segment => segment.kind === 'audible') }));
  let lastOrder = -1, orderRegressions = 0, unexpectedFrames = 0;
  const ordered = decoded.runs.map(run => {
    const index = expected.findIndex(segment => run.sourceStart >= segment.start && run.sourceStart + run.frames <= segment.end);
    if (index < 0) unexpectedFrames += run.frames;
    else { if (index < lastOrder) orderRegressions++; lastOrder = Math.max(lastOrder, index); }
    return { ...run, order: index < 0 ? null : expected[index].order };
  });
  const orders = expected.map(segment => {
    const runs = ordered.filter(run => run.order === segment.order), measured = coverage(runs, segment);
    const sourceRegressions = runs.slice(1).filter((run, index) => run.sourceStart < runs[index].sourceStart + runs[index].frames).length;
    const recordingGaps = runs.slice(1).flatMap((run, index) => {
      const start = runs[index].recordingStart + runs[index].frames;
      return run.recordingStart > start ? [{ start, end: run.recordingStart }] : [];
    });
    return { order: segment.order, expectedFrames: segment.end - segment.start, ...measured, recordingGaps, sourceRegressions,
      passed: measured.missingFrames === 0 && measured.repeatedFrames === 0 && recordingGaps.length === 0 && sourceRegressions === 0 };
  });
  return { format, ...decoded, runs: ordered, orders, unexpectedFrames, orderRegressions,
    quietSampleIdentityObservable: false,
    passed: decoded.nonfiniteSamples === 0 && decoded.unidentifiedNonzeroFrames === 0
      && unexpectedFrames === 0 && orderRegressions === 0 && orders.every(order => order.passed) };
}
