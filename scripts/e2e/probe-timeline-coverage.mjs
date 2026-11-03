import assert from 'node:assert/strict';

// Intervals describe observed coordinates. Missing retained rows alone do not
// prove that a callback or a publication never occurred.
function coverage(rows, start, end) {
  const intervals = rows.map(({ sample, frames }) => {
    assert.ok(Number.isSafeInteger(sample + frames));
    return [Math.max(start, sample), Math.min(end, sample + frames)];
  }).filter(([first, last]) => first < last).sort((a, b) => a[0] - b[0]);
  let cursor = start, total = 0, covered = 0;
  const missing = [];
  for (const [first, last] of intervals) {
    total += last - first;
    if (first > cursor) missing.push({ start: cursor, end: first });
    covered += Math.max(0, last - Math.max(cursor, first));
    cursor = Math.max(cursor, last);
  }
  if (cursor < end) missing.push({ start: cursor, end });
  return { frames: end - start, coveredFrames: covered, missingFrames: end - start - covered,
    duplicateFrames: total - covered, missing };
}

export function probeTimelineCoverage(rows, trace, { start, end, sourceClient }) {
  assert.ok(Number.isSafeInteger(start) && Number.isSafeInteger(end) && end > start);
  const native = render => trace.rows.filter(row => row[1] === render && row[4] === 1)
    .map(row => ({ sample: row[2], frames: row[3], client: row[8] ?? null }));
  return {
    outputCallbacks: coverage(rows.filter(row => row.side === 'output'), start, end),
    inputCallbacks: coverage(rows.filter(row => row.side === 'input'), start, end),
    sourceNativeWrites: sourceClient === null ? null
      : coverage(native(1).filter(row => row.client === sourceClient), start, end),
    nativeReads: coverage(native(0), start, end),
    limitations: [
      'Callback coordinates and retained native coordinates have separate coverage; neither reconstructs missing source PCM.',
      'Source native coverage requires one writer identified by the reference hashes. Missing epochs and trace retention limit attribution.',
    ],
  };
}
