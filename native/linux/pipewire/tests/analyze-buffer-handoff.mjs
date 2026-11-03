// Off-process verification; no reconstruction or repair of production PCM.
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { analyze as analyzeSequence } from './analyze-source-sequence.mjs';

export function payloadHash(bytes) {
  let hash = 0xcbf29ce484222325n;
  for (const byte of bytes) hash = BigInt.asUintN(64, (hash ^ BigInt(byte)) * 0x100000001b3n);
  return hash.toString(16).padStart(16, '0');
}

export function verifyJournal(document) {
  const { journal } = document;
  assert.equal(document.liveStreamsAtExit, 0, 'Native stream was not destroyed');
  assert.equal(document.untrackedStreams, 0, 'Direction table exhausted');
  assert.equal(journal.dropped, 0, 'Journal exhausted');
  assert.equal(journal.incomplete, 0, 'Writer had not published at finalization');
  assert.equal(journal.reserved, journal.records.length);
  const records = journal.records;
  assert.equal(records[0]?.operation, 'Open');
  assert.equal(records.at(-1)?.operation, 'Close');
  assert.equal(new Set(records.map(record => record.stream)).size, 1, 'Expected one private stream');
  for (const record of records) {
    assert(Number.isSafeInteger(record.monotonic_ns));
    assert.equal(record.native_time_result, 0);
    assert.equal(record.result, 0, 'Native handoff failed');
  }
  for (let index = 1; index < records.length; index += 1) {
    assert(records[index].monotonic_ns >= records[index - 1].monotonic_ns,
      `Native monotonic timestamp moved backwards at record ${index}`);
  }
  const input = records.some(record => record.operation === 'Dequeue' && record.payload.valid);
  const direction = input ? 'input' : 'output';
  const buffers = [];
  const handoffs = [];
  for (let index = 1; index < records.length - 1; index += 2) {
    const received = records[index], returned = records[index + 1];
    assert.equal(received.operation, 'Dequeue');
    assert.equal(returned.operation, 'Queue');
    assert.equal(returned.stream, received.stream, 'Native handoff changed stream identity');
    assert.equal(returned.name, received.name, 'Native handoff changed stream name');
    assert.equal(received.buffer_present, true);
    assert.equal(returned.buffer_present, true);
    assert.equal(returned.payload.valid, true);
    assert.equal(returned.payload.bytes, 1024, 'This analysis requires 128-frame stereo buffers');
    assert(returned.monotonic_ns >= received.monotonic_ns,
      `Native queue timestamp precedes dequeue at pair ${handoffs.length}`);
    if (input) assert.deepEqual(received.payload, returned.payload, 'Input mutated before return');
    else assert.equal(received.payload.valid, false, 'Uninitialized output PCM was inspected');
    handoffs.push({ direction, stream: received.stream, name: received.name,
      dequeueMonotonicNs: received.monotonic_ns, queueMonotonicNs: returned.monotonic_ns,
      ownershipDwellNs: returned.monotonic_ns - received.monotonic_ns });
    buffers.push(input ? received : returned);
  }
  return { direction, stream: records[0].stream, name: records[0].name, buffers, handoffs };
}

function captureStart(bytes, delivered) {
  // Newer native clients can return multiple identical silent startup blocks.
  // Align using the first tagged block, then verify the entire prefix as well;
  // neither trimming it nor accepting any matching endpoint proves conservation.
  let anchor = 0;
  while (anchor + 1024 <= bytes.length && decode(bytes.readUInt32LE(anchor + 4)) === null) anchor += 1024;
  assert(anchor + 1024 <= bytes.length, 'Capture contains no complete tagged anchor');
  const first = bytes.readUInt32LE(anchor + 4), firstHash = payloadHash(bytes.subarray(anchor, anchor + 1024));
  const candidates = delivered.flatMap((record, index) => record.payload.first[1] === first
    && record.payload.hash === firstHash ? [index] : []);
  assert.equal(candidates.length, 1, 'Capture anchor must identify exactly one delivered block');
  const start = candidates[0] - anchor / 1024;
  assert(start >= 0, 'Startup PCM precedes the observed native delivery');
  return start;
}

export function compareCapture(bytes, delivered) {
  assert.equal(bytes.length % 8, 0);
  assert(bytes.length >= 1024);
  const start = captureStart(bytes, delivered), blocks = Math.floor(bytes.length / 1024);
  for (let block = 0; block < blocks; block++) {
    const expected = delivered[start + block];
    assert(expected, 'Capture exceeds observed native delivery');
    assert.equal(payloadHash(bytes.subarray(block * 1024, (block + 1) * 1024)), expected.payload.hash,
      `Captured block ${block} differs from consecutive native handoff ${start + block}`);
  }
  const trailingFrames = bytes.length / 8 - blocks * 128;
  return { firstDeliveredBlock: start, checksummedFrames: blocks * 128,
    trailingFramesNotChecksummed: trailingFrames, contiguousDeliveredBlocks: blocks };
}

const decode = bits => (bits & 0xff800000) === 0x3d000000 ? bits & 0x7fffff : null;

const numberOrNull = value => Number.isSafeInteger(value) ? value : null;

function context(record) {
  if (!record) return null;
  const first = record.payload?.first?.[1];
  const last = record.payload?.last?.[1];
  return { stream: numberOrNull(record.stream), streamName: record.name ?? null,
    monotonicNs: record.monotonic_ns, nativeTicks: record.native_ticks,
    firstTag: first === undefined ? null : decode(first),
    lastTag: last === undefined ? null : decode(last), hash: record.payload?.hash };
}

function timingContext(record, previous, handoff, bufferIndex) {
  if (!record) return null;
  if (handoff) {
    assert.equal(handoff.stream, record.stream, 'Ownership dwell changed stream identity');
    assert.equal(handoff.name, record.name, 'Ownership dwell changed stream name');
    assert.equal(handoff.dequeueMonotonicNs, record.monotonic_ns,
      'Ownership dwell is not paired with this dequeue');
    assert(handoff.queueMonotonicNs >= handoff.dequeueMonotonicNs,
      'Ownership queue timestamp precedes its dequeue');
    assert.equal(handoff.ownershipDwellNs,
      handoff.queueMonotonicNs - handoff.dequeueMonotonicNs,
      'Ownership dwell does not match its observed timestamps');
  }
  const previousMonotonicNs = previous?.monotonic_ns;
  const previousNativeTicks = previous?.native_ticks;
  const interDequeueGap = Number.isSafeInteger(record.monotonic_ns)
    && Number.isSafeInteger(previousMonotonicNs)
    ? record.monotonic_ns - previousMonotonicNs : null;
  assert(interDequeueGap === null || interDequeueGap >= 0,
    `Native dequeue timestamp moved backwards at buffer ${bufferIndex}`);
  const tickDelta = Number.isSafeInteger(record.native_ticks)
    && Number.isSafeInteger(previousNativeTicks)
    && Number.isSafeInteger(record.rate_num) && record.rate_num > 0
    && Number.isSafeInteger(record.rate_denom) && record.rate_denom > 0
    && record.rate_num === previous?.rate_num && record.rate_denom === previous?.rate_denom
    ? record.native_ticks - previousNativeTicks : null;
  return { ...context(record), nativeReportNs: numberOrNull(record.native_report_ns),
    rateNum: numberOrNull(record.rate_num), rateDenom: numberOrNull(record.rate_denom),
    bufferIndex, interDequeueGapNs: interDequeueGap, nativeTickDelta: tickDelta,
    ownership: handoff ? { direction: handoff.direction, stream: handoff.stream,
      name: handoff.name, dequeueMonotonicNs: handoff.dequeueMonotonicNs,
      queueMonotonicNs: handoff.queueMonotonicNs,
      dequeueToQueueDwellNs: handoff.ownershipDwellNs } : null };
}

function sourceQueueContext(tagged, expected) {
  if (expected === null) return { previous: null, current: null, next: null };
  const containing = tagged.findIndex(value => value.firstTag <= expected && value.lastTag >= expected);
  if (containing >= 0) {
    return { previous: tagged[containing - 1] ?? null, current: tagged[containing],
      next: tagged[containing + 1] ?? null };
  }
  const insertion = tagged.findIndex(value => value.firstTag > expected);
  const next = insertion < 0 ? null : tagged[insertion];
  const previous = insertion < 0 ? tagged.at(-1) : tagged[insertion - 1];
  return { previous: previous ?? null, current: null, next };
}

export function explain(events, delivered, source, match, trim, handoffs = []) {
  const tagged = source.map(context).filter(value => value && value.firstTag !== null && value.lastTag !== null);
  return events.map(event => {
    const absoluteStart = event.sampleIndex + trim;
    const index = match.firstDeliveredBlock + Math.floor(absoluteStart / 128);
    const affectedFrames = event.kind === 'invalid' ? event.length : 1;
    assert(Number.isSafeInteger(affectedFrames) && affectedFrames > 0,
      'Continuity event must identify at least one affected frame');
    const lastAffectedIndex = match.firstDeliveredBlock
      + Math.floor((absoluteStart + affectedFrames - 1) / 128);
    const arrival = delivered[index];
    const at = deliveryIndex => timingContext(delivered[deliveryIndex], delivered[deliveryIndex - 1],
      handoffs[deliveryIndex], deliveryIndex);
    const affected = Array.from({ length: lastAffectedIndex - index + 1 }, (_, offset) =>
      at(index + offset));
    const sourceContext = sourceQueueContext(tagged, event.expected);
    const generated = sourceContext.current;
    const sourceQueueMinusDeliveryNs = generated && arrival
      && Number.isSafeInteger(generated.monotonicNs) && Number.isSafeInteger(arrival.monotonic_ns)
      ? generated.monotonicNs - arrival.monotonic_ns : null;
    return { ...event, nativeDeliveryIndex: index, nativeDelivery: context(arrival),
      sourceQueueContainingExpected: generated,
      sourceQueueMinusDeliveryNs,
      captureDeliveries: { previous: at(index - 1), current: at(index), affected,
        next: at(lastAffectedIndex + 1) },
      sourceQueueContext: sourceContext };
  });
}

export function analyze(directory) {
  const files = readdirSync(directory).filter(file => /^buffer-observer-\d+\.json$/.test(file));
  assert.equal(files.length, 3, 'Require source, MixRelay and independent recorder journals');
  const clients = files.map(file => {
    const document = JSON.parse(readFileSync(join(directory, file)));
    return { file, document, ...verifyJournal(document) };
  });
  const client = name => {
    const matching = clients.filter(client => name(client.document.journal.records[0].name));
    assert.equal(matching.length, 1);
    return matching[0];
  };
  const source = client(name => name === 'MixRelay application fixture');
  const native = client(name => name.startsWith('io.mixrelay.fixture.capture.'));
  const independent = client(name => name === 'pw-record');
  for (const [index, record] of source.buffers.entries()) {
    assert.equal(decode(record.payload.first[1]), index * 128, 'Source queue skipped frames');
    assert.equal(decode(record.payload.last[1]), index * 128 + 127);
  }
  const nativeMatch = compareCapture(readFileSync(join(directory, 'native-capture.f32')), native.buffers);
  const independentMatch = compareCapture(readFileSync(join(directory, 'independent-capture.f32')), independent.buffers);
  const sequence = analyzeSequence(directory);
  return { directory, scope: '128-frame no-SRC stereo fixture; hashes do not qualify other routes',
    timingSemantics: {
      interDequeueGapNs: 'difference between consecutive native dequeue observations; not scheduler attribution',
      dequeueToQueueDwellNs: 'paired queue-hook timestamp minus dequeue-hook timestamp',
    },
    journals: clients.map(({ file, document, direction, stream, name, buffers, handoffs }) => ({ file,
      direction, stream, name,
      records: document.journal.reserved, buffers: buffers.length,
      handoffs: handoffs.length,
      dropped: document.journal.dropped, incomplete: document.journal.incomplete,
      liveStreamsAtExit: document.liveStreamsAtExit, untrackedStreams: document.untrackedStreams })),
    sourceQueuedFrames: source.buffers.length * 128,
    nativeMatch, independentMatch,
    nativeEvents: explain(sequence.native, native.buffers, source.buffers, nativeMatch, 4800, native.handoffs),
    independentEvents: explain(sequence.independent, independent.buffers, source.buffers, independentMatch, 4800,
      independent.handoffs),
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  assert(process.argv.length > 2, 'Expected retained lab directories');
  console.log(JSON.stringify(process.argv.slice(2).map(analyze), null, 2));
}
