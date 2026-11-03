// Read-only analysis of the tagged, little-endian f32 stereo Linux lab recordings.
import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const mask = 0x7fffff;
const prefix = 0x3d000000;

export function sequenceEvents(bytes, trimStart = 0, trimEnd = 0) {
  if (bytes.length % 8 !== 0 || !Number.isInteger(trimStart) || !Number.isInteger(trimEnd)
      || trimStart < 0 || trimEnd < 0 || trimStart + trimEnd > bytes.length / 8) {
    throw new Error('Expected complete little-endian f32 stereo frames and valid trims');
  }
  const events = [];
  let previous = null;
  let invalid = null;
  for (let index = trimStart; index < bytes.length / 8 - trimEnd; index++) {
    const bits = bytes.readUInt32LE(index * 8 + 4);
    const expected = previous === null ? null : (previous + 1) & mask;
    if ((bits & ~mask) !== prefix) {
      invalid ??= { kind: 'invalid', sampleIndex: index - trimStart, expected, length: 0 };
      invalid.length++;
      continue;
    }
    const actual = bits & mask;
    if (invalid) {
      events.push({ ...invalid, resumedAt: actual });
      invalid = null;
    }
    if (expected !== null && expected !== actual) {
      const modularDelta = (actual - expected) & mask;
      events.push({ kind: 'jump', sampleIndex: index - trimStart, expected, actual,
        signedTagStep: modularDelta > (mask + 1) / 2 ? modularDelta - mask - 1 : modularDelta });
    }
    previous = actual;
  }
  if (invalid) events.push({ ...invalid, resumedAt: null });
  return events;
}

const identity = event => event.kind === 'jump'
  ? `jump:${event.expected}:${event.actual}`
  : `invalid:${event.expected}:${event.resumedAt}:${event.length}`;

export function compareEvents(native, independent) {
  // Position-free keys compare source identity, not unsynchronized capture indices.
  // Pair as a multiset so repeated evidence cannot satisfy several events at once.
  const remaining = [...independent];
  const common = [], nativeOnly = [];
  for (const event of native) {
    const match = remaining.findIndex(other => identity(other) === identity(event));
    if (match < 0) nativeOnly.push(event);
    else common.push({ native: event, independent: remaining.splice(match, 1)[0] });
  }
  return { common, nativeOnly, independentOnly: remaining };
}

function verifySummary(events, summary) {
  const invalid = events.filter(event => event.kind === 'invalid')
    .reduce((count, event) => count + event.length, 0);
  const jumps = events.filter(event => event.kind === 'jump').length;
  if (invalid !== summary.invalid_samples || jumps !== summary.discontinuities) {
    throw new Error('Independent raw-PCM analysis disagrees with the Rust measurement');
  }
}

function sourceContext(events, frame) {
  if (frame === null) return null;
  const index = events.findIndex(event => frame >= event.first_frame
    && frame < event.first_frame + event.produced_frames);
  if (index < 0) return null;
  return events.slice(Math.max(0, index - 1), index + 2).map(event => ({
    ...event, gapBeforeNs: event.sequence > 1
      ? event.elapsed_ns - events[event.sequence - 2].elapsed_ns : null,
  }));
}

export function analyze(directory) {
  const evidence = JSON.parse(readFileSync(join(directory, 'independent-continuity.json')));
  const source = JSON.parse(readFileSync(join(directory, evidence.source.traceFile)));
  if (!evidence.frameSequence.applicable || source.right_channel !== 'frame-sequence-f32-23bit'
      || source.sample_rate !== 48000 || source.dropped_records !== 0) {
    throw new Error('Frame identity requires the tagged, unconverted 48 kHz case and complete trace');
  }
  const native = sequenceEvents(readFileSync(join(directory, 'native-capture.f32')), 4800);
  const independent = sequenceEvents(readFileSync(join(directory, 'independent-capture.f32')), 4800, 4800);
  verifySummary(native, evidence.frameSequence.native);
  verifySummary(independent, evidence.frameSequence.independent);
  const sourcePositionBreaks = source.events.slice(1).filter((event, index) =>
    event.first_frame !== source.events[index].first_frame + source.events[index].produced_frames).length;
  const enriched = events => events.map(event => ({ ...event,
    sourceCallbacks: sourceContext(source.events, event.expected),
  }));
  return { directory, scope: 'frame-tag identity, modulo 2^23; not absolute latency or SRC evidence',
    sourceCallbacks: source.events.length, sourcePositionBreaks,
    sourceUnfilledRequests: source.events.filter(event => event.requested_frames !== null
      && event.requested_frames > event.produced_frames).length,
    native: enriched(native), independent: enriched(independent),
    ...compareEvents(native, independent) };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv.length < 3) throw new Error('Expected one or more retained lab directories');
  console.log(JSON.stringify(process.argv.slice(2).map(analyze), null, 2));
}
