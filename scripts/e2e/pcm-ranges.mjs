import assert from 'node:assert/strict';

export function mergePcmRanges(ranges) {
  const merged = [];
  for (const { start, end } of ranges.toSorted((a, b) => a.start - b.start)) {
    assert.ok(Number.isSafeInteger(start) && Number.isSafeInteger(end) && start >= 0 && end > start);
    const previous = merged.at(-1);
    if (previous && start <= previous.end) previous.end = Math.max(previous.end, end);
    else merged.push({ start, end });
  }
  return merged;
}

export function intersectPcmRanges(ranges, start, end) {
  return mergePcmRanges(ranges.flatMap(range => {
    const first = Math.max(start, range.start), last = Math.min(end, range.end);
    return first < last ? [{ start: first, end: last }] : [];
  }));
}

export const pcmRangeFrames = ranges => ranges.reduce((total, range) => total + range.end - range.start, 0);

export function subtractPcmRanges(expected, observed) {
  const result = [], covered = mergePcmRanges(observed);
  for (const range of mergePcmRanges(expected)) {
    let cursor = range.start;
    for (const other of covered) {
      if (other.end <= cursor || other.start >= range.end) continue;
      if (other.start > cursor) result.push({ start: cursor, end: other.start });
      cursor = Math.max(cursor, Math.min(range.end, other.end));
    }
    if (cursor < range.end) result.push({ start: cursor, end: range.end });
  }
  return result;
}
