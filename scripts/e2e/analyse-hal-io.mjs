import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';
import { analyseProbeReference } from './probe-pcm-reference.mjs';

const integer = value => Number.isSafeInteger(value) && value >= 0;

export function analyseHalIo(trace, signalWindow = null) {
  assert.equal(trace.version, 5, 'Unsupported HAL trace version');
  assert.equal(trace.capacity, 8192, 'Unexpected HAL trace capacity');
  const admission = trace.admission;
  assert.ok(admission, 'Missing HAL admission counters');
  for (const field of ['read', 'write', 'timestamp', 'extent', 'stream', 'frozen', 'inactive', 'native'])
    assert.ok(integer(admission[field]), `Invalid HAL admission counter ${field}`);
  assert.match(admission.lastRejectedTimestampBits, /^[0-9a-f]{16}$/);
  for (const field of ['nullBuffer', 'oversizedFrames'])
    assert.ok(integer(admission[field]), `Missing native rejected extent classification ${field}`);
  const extent = admission.lastRejectedExtent;
  assert.ok(extent && integer(extent.frames) && extent.frames <= 0xffffffff, 'Missing native rejected extent');
  assert.equal(typeof extent.bufferPresent, 'boolean');
  assert.equal(typeof extent.render, 'boolean');
  assert.ok(integer(trace.issued) && integer(trace.dropped) && trace.dropped <= trace.issued);
  assert.ok(Array.isArray(trace.rows) && trace.rows.length <= trace.capacity);
  if (signalWindow) assert.ok(integer(signalWindow.start) && integer(signalWindow.end) && signalWindow.end > signalWindow.start);
  const sequences = new Set();
  const rows = trace.rows.map(row => {
    assert.ok(Array.isArray(row) && row.length === 11 && row.slice(0, 7).every(integer));
    const [sequence, render, frame, frames, accepted, available, zeroFrames, hash] = row;
    assert.ok(sequence > 0 && sequence <= trace.issued && !sequences.has(sequence), 'Invalid or repeated observation sequence');
    assert.ok(render <= 1 && accepted <= 1 && frames <= 0xffffffff && available <= frames && zeroFrames <= frames);
    assert.ok(Number.isSafeInteger(frame + frames) && typeof hash === 'string' && /^[0-9a-f]{16}$/.test(hash));
    const client = row[8];
    assert.ok(integer(client) && client <= 0xffffffff);
    sequences.add(sequence);
    const processing = { hostStart: row[9], hostEnd: row[10] };
    assert.ok(integer(processing.hostStart) && integer(processing.hostEnd) && processing.hostEnd >= processing.hostStart, 'Invalid processing host counters');
    const overlap = !signalWindow ? 'unknown' : frame >= signalWindow.end || frame + frames <= signalWindow.start
      ? 'outside' : frame >= signalWindow.start && frame + frames <= signalWindow.end ? 'full' : 'partial';
    return { sequence, client, render: render === 1, frame, frames, accepted: accepted === 1, available, zeroFrames, hash, signalOverlap: overlap, processing };
  }).sort((a, b) => a.sequence - b.sequence);
  const writeClients = new Set(rows.filter(row => row.render).map(row => row.client));
  const writes = new Map();
  const suppliedHashDifferences = [];
  let exactCoordinatePairs = 0, unpairedCaptureRows = 0;
  for (const row of rows) {
    const key = `${row.frame}:${row.frames}`;
    if (row.render) { writes.set(key, row); continue; }
    const write = writes.get(key);
    if (!write) { unpairedCaptureRows++; continue; }
    exactCoordinatePairs++;
    if (writeClients.size <= 1 && write.accepted && row.accepted && row.available === row.frames && write.hash !== row.hash)
      suppliedHashDifferences.push({ write, capture: row });
  }
  const visible = row => row.signalOverlap !== 'outside';
  const recentStart = Math.max(1, trace.issued - trace.capacity + 1);
  const recentRows = rows.filter(row => row.sequence >= recentStart).length;
  return {
    admission,
    signalWindow,
    retention: { issued: trace.issued, capacity: trace.capacity, writerDropped: trace.dropped,
      retainedRows: rows.length, notRetainedRecentRows: Math.min(trace.issued, trace.capacity) - recentRows,
      observationsBeyondCapacity: Math.max(0, trace.issued - trace.capacity) },
    writeRejections: rows.filter(row => row.render && !row.accepted && visible(row)),
    writeSilence: rows.filter(row => row.render && row.zeroFrames > 0 && visible(row)),
    captureShortReads: rows.filter(row => !row.render && row.available < row.frames && visible(row)),
    suppliedCaptureSilence: rows.filter(row => !row.render && row.accepted && row.available === row.frames && row.zeroFrames > 0 && visible(row)),
    exactCoordinatePairs, unpairedCaptureRows, suppliedHashDifferences,
    writeClients: [...writeClients],
    captureHashComparison: writeClients.size <= 1 ? "single_output" : "inapplicable_multiple_client_mix",
    emptyTransfers: rows.filter(row => row.frames === 0).length,
    limitations: [
      'Observation sequence is assigned after native IO; it does not establish publication order across callbacks.',
      'Rows do not include epochs. Equal coordinates alone do not prove the same timeline.',
      'Partial signal overlap does not locate zeros or unavailable frames inside that overlap.',
      'Unpaired or unretained rows do not establish that native IO was absent.',
      'Admission counters cover SDK timestamp/extent and registry stream/freeze/inactive/native outcomes; counts are not an atomic transaction and do not locate a rejection in time.',
    ],
  };
}

export function probeSignalWindow(metrics, timing) {
  const first = timing.split('\n').find(line => /^output,/.test(line))?.split(',');
  if (!first || first.length !== 8 || !integer(metrics.output?.rate) || metrics.output.rate === 0 || !integer(metrics.signalFrames) || metrics.signalFrames === 0) return null;
  const origin = Number(first[2]) - Number(first[1]);
  if (!integer(origin) || !(Number(first[5]) & 1)) return null;
  const start = origin + Math.trunc(metrics.output.rate * 0.1);
  return { start, end: start + metrics.signalFrames };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values, positionals } = parseArgs({ allowPositionals: true,
    options: { help: { type: 'boolean' }, output: { type: 'string' } } });
  if (values.help) console.log('node scripts/e2e/analyse-hal-io.mjs EVIDENCE_DIRECTORY [--output FILE]\nReads native-hal-io.json and original probe JSON/timing offline. Reports observations and coverage limits; it does not change the PCM verdict or access devices.');
  else {
    assert.equal(positionals.length, 1, 'Supply one fresh or upgrade evidence directory');
    const directory = positionals[0];
    const metrics = JSON.parse(readFileSync(join(directory, 'native-probe.stdout.json'), 'utf8'));
    assert.equal(metrics.error, undefined, 'Probe failed before producing a measurement');
    const trace = JSON.parse(readFileSync(join(directory, 'native-hal-io.json'), 'utf8'));
    const timing = readFileSync(join(directory, 'native-probe.stderr.log'), 'utf8');
    const report = analyseHalIo(trace, probeSignalWindow(metrics, timing));
    report.probeReference = analyseProbeReference(trace, metrics, timing, readFileSync(join(directory, 'native-signal.f32')));
    report.originalPcmVerdict = Object.fromEntries(['complete', 'missingFrames', 'repeatedFrames', 'corruptFrames', 'nonfiniteSamples'].map(field => [field, metrics[field]]));
    const json = JSON.stringify(report, null, 2) + '\n';
    if (values.output) writeFileSync(values.output, json, { flag: 'wx' });
    else process.stdout.write(json);
  }
}
