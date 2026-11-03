import { test } from 'node:test';
import assert from 'node:assert/strict';
import { payloadHash, compareCapture, explain, verifyJournal } from './analyze-buffer-handoff.mjs';

const block = position => {
  const bytes = Buffer.alloc(1024);
  for (let index = 0; index < 128; index++) bytes.writeUInt32LE(0x3d000000 | position + index, index * 8 + 4);
  return bytes;
};
const delivered = (bytes, timing = {}) => ({ monotonic_ns: timing.monotonic_ns ?? 100,
  native_ticks: timing.native_ticks === undefined ? 128 : timing.native_ticks,
  native_report_ns: timing.native_report_ns ?? 90,
  rate_num: timing.rate_num ?? 1, rate_denom: timing.rate_denom ?? 48000,
  payload: { first: [0, bytes.readUInt32LE(4)], last: [0, bytes.readUInt32LE(bytes.length - 4)],
    hash: payloadHash(bytes) } });

const sourceRecord = (first, timing = {}) => {
  const bytes = block(first);
  return delivered(bytes, timing);
};

test('capture requires consecutive native blocks, not merely matching endpoints or totals', () => {
  assert.equal(payloadHash(Buffer.alloc(1024)), '51d88627df287325');
  const blocks = [0, 128, 256, 384].map(block);
  const records = blocks.map(delivered);
  assert.deepEqual(compareCapture(Buffer.concat(blocks.slice(1)), records), {
    firstDeliveredBlock: 1, checksummedFrames: 384, trailingFramesNotChecksummed: 0, contiguousDeliveredBlocks: 3,
  });
  assert.throws(() => compareCapture(Buffer.concat([blocks[1], blocks[3]]), records));
  assert.throws(() => compareCapture(Buffer.concat([blocks[1], blocks[1]]), records));
  const changed = Buffer.from(blocks[2]); changed[400] ^= 1;
  assert.throws(() => compareCapture(Buffer.concat([blocks[1], changed]), records));
  assert.equal(compareCapture(Buffer.concat([blocks[1], blocks[2].subarray(0, 512)]), records)
    .trailingFramesNotChecksummed, 64);
  assert.throws(() => compareCapture(blocks[1], [records[1], records[1]]));
});

test('incomplete, overflowing and unclosed native observations cannot be accepted', () => {
  const base = { liveStreamsAtExit: 0, untrackedStreams: 0,
    journal: { dropped: 0, incomplete: 0, reserved: 0, records: [] } };
  for (const changed of [ { ...base, liveStreamsAtExit: 1 }, { ...base, untrackedStreams: 1 },
    { ...base, journal: { ...base.journal, dropped: 1 } },
    { ...base, journal: { ...base.journal, incomplete: 1 } }, base ]) {
    assert.throws(() => verifyJournal(changed));
  }
});

test('silent startup alignment still checks every prefix block and requires a unique anchor', () => {
  const silence = Buffer.alloc(1024), tagged = block(128);
  const records = [silence, silence, silence, tagged].map(delivered);
  const capture = Buffer.concat([silence, silence, tagged]);
  assert.deepEqual(compareCapture(capture, records), {
    firstDeliveredBlock: 1, checksummedFrames: 384, trailingFramesNotChecksummed: 0, contiguousDeliveredBlocks: 3,
  });
  const changed = Buffer.from(capture); changed[0] = 1;
  assert.throws(() => compareCapture(changed, records), /Captured block 0/);
  assert.throws(() => compareCapture(capture, [delivered(tagged)]), /precedes/);
  assert.throws(() => compareCapture(capture, [...records, delivered(tagged)]), /exactly one/);
  assert.throws(() => compareCapture(silence, records), /no complete tagged anchor/);
  assert.throws(() => compareCapture(Buffer.concat([silence, tagged.subarray(0, 512)]), records));
});

test('paired native input handoffs preserve payload and reject error or mutation', () => {
  const payload = { ...delivered(block(128)).payload, valid: true, bytes: 1024, stride: 8,
    last: [0, 0x3d0000ff] };
  const record = operation => ({ operation, stream: 1, monotonic_ns: 100, native_time_result: 0,
    result: 0, buffer_present: true, payload: structuredClone(payload) });
  const document = { liveStreamsAtExit: 0, untrackedStreams: 0,
    journal: { dropped: 0, incomplete: 0, reserved: 4,
      records: ['Open', 'Dequeue', 'Queue', 'Close'].map(record) } };
  const verified = verifyJournal(document);
  assert.equal(verified.direction, 'input');
  assert.equal(verified.buffers.length, 1);
  assert.deepEqual(verified.handoffs[0], {
    direction: 'input', stream: 1, name: undefined,
    dequeueMonotonicNs: 100, queueMonotonicNs: 100, ownershipDwellNs: 0,
  });
  const changed = structuredClone(document);
  changed.journal.records[2].payload.hash = '0000000000000000';
  assert.throws(() => verifyJournal(changed));
  const failed = structuredClone(document);
  failed.journal.records[2].result = -1;
  assert.throws(() => verifyJournal(failed));
  const output = structuredClone(document);
  output.journal.records[1].payload.valid = false;
  assert.equal(verifyJournal(output).direction, 'output');
  assert.equal(verifyJournal(output).buffers[0].operation, 'Queue');
  assert.equal(verifyJournal(output).handoffs[0].direction, 'output');
});

test('native ownership dwell uses only paired monotonic dequeue and queue observations', () => {
  const payload = { ...delivered(block(128)).payload, valid: true, bytes: 1024, stride: 8,
    last: [0, 0x3d0000ff] };
  const record = (operation, monotonic_ns) => ({ operation, stream: 17, name: 'pw-record',
    monotonic_ns, native_time_result: 0, result: 0, buffer_present: true,
    payload: structuredClone(payload) });
  const document = { liveStreamsAtExit: 0, untrackedStreams: 0,
    journal: { dropped: 0, incomplete: 0, reserved: 4,
      records: [record('Open', 50), record('Dequeue', 100), record('Queue', 160), record('Close', 170)] } };
  const verified = verifyJournal(document);
  assert.deepEqual(verified.handoffs[0], {
    direction: 'input', stream: 17, name: 'pw-record',
    dequeueMonotonicNs: 100, queueMonotonicNs: 160, ownershipDwellNs: 60,
  });

  const reversed = structuredClone(document);
  reversed.journal.records[2].monotonic_ns = 99;
  assert.throws(() => verifyJournal(reversed), /monotonic timestamp moved backwards/);
  const missing = structuredClone(document);
  missing.journal.records.splice(2, 1);
  missing.journal.reserved -= 1;
  assert.throws(() => verifyJournal(missing), /Queue/);
});

test('explained delivery context guards the first and last capture boundaries', () => {
  const records = [0, 128, 256].map((position, index) => delivered(block(position), {
    monotonic_ns: 1000 + index * 2500, native_ticks: index === 1 ? 7 : index * 128,
    native_report_ns: 900 + index * 2500,
  }));
  const source = [0, 128, 256].map(sourceRecord);
  const [first, last] = explain([
    { kind: 'jump', sampleIndex: 0, expected: 0, actual: 128 },
    { kind: 'jump', sampleIndex: 256, expected: 256, actual: 384 },
  ], records, source, { firstDeliveredBlock: 0 }, 0);
  assert.equal(first.captureDeliveries.previous, null);
  assert.equal(first.captureDeliveries.current.nativeReportNs, 900);
  assert.equal(first.captureDeliveries.current.interDequeueGapNs, null);
  assert.equal(first.captureDeliveries.next.nativeTickDelta, 7);
  assert.equal(last.captureDeliveries.previous.interDequeueGapNs, 2500);
  assert.equal(last.captureDeliveries.current.nativeTickDelta, 249);
  assert.equal(last.captureDeliveries.next, null);
  assert.equal(first.sourceQueueContext.previous, null);
  assert.equal(first.sourceQueueContext.current.firstTag, 0);
  assert.equal(first.sourceQueueContext.current.lastTag, 127);
  assert.equal(first.sourceQueueContext.current.hash, payloadHash(block(0)));
  assert.equal(first.sourceQueueContext.next.firstTag, 128);
  assert.equal(first.sourceQueueContext.next.lastTag, 255);
  assert.equal(first.sourceQueueContext.next.hash, payloadHash(block(128)));
});

test('source queue context retains neighbors when the expected tag is absent', () => {
  const source = [sourceRecord(0), sourceRecord(256)];
  const records = [delivered(block(0)), delivered(block(128)), delivered(block(256))];
  const [event] = explain([{ kind: 'jump', sampleIndex: 128, expected: 128, actual: 256 }],
    records, source, { firstDeliveredBlock: 0 }, 0);
  assert.equal(event.sourceQueueContainingExpected, null);
  assert.equal(event.sourceQueueContext.current, null);
  assert.equal(event.sourceQueueContext.previous.firstTag, 0);
  assert.equal(event.sourceQueueContext.next.firstTag, 256);
});

test('repeated native ticks remain zero deltas while absent ticks stay null', () => {
  const records = [
    delivered(block(0), { monotonic_ns: 100, native_ticks: 4 }),
    delivered(block(128), { monotonic_ns: 200, native_ticks: 4 }),
    delivered(block(256), { monotonic_ns: 300, native_ticks: null }),
  ];
  const source = [0, 128, 256].map(sourceRecord);
  const [, repeated, absent] = explain([
    { kind: 'jump', sampleIndex: 0, expected: 0, actual: 128 },
    { kind: 'jump', sampleIndex: 128, expected: 128, actual: 256 },
    { kind: 'jump', sampleIndex: 256, expected: 256, actual: 384 },
  ], records, source, { firstDeliveredBlock: 0 }, 0);
  assert.equal(repeated.captureDeliveries.current.nativeTickDelta, 0);
  assert.equal(absent.captureDeliveries.current.nativeTickDelta, null);
});

test('invalid source tags cannot match zero and changed tick units are not subtracted', () => {
  const records = [delivered(block(0)), delivered(block(128), { rate_denom: 44100 })];
  const source = [delivered(Buffer.alloc(1024)), { payload: {} }, sourceRecord(128)];
  const [event] = explain([{ kind: 'jump', sampleIndex: 128, expected: 0, actual: 128 }],
    records, source, { firstDeliveredBlock: 0 }, 0);
  assert.equal(event.sourceQueueContainingExpected, null);
  assert.equal(event.sourceQueueMinusDeliveryNs, null);
  assert.equal(event.sourceQueueContext.previous, null);
  assert.equal(event.sourceQueueContext.next.firstTag, 128);
  assert.equal(event.captureDeliveries.current.nativeTickDelta, null);
});

test('silent spans report preceding and affected buffer dwell separately from dequeue gaps', () => {
  const blocks = [block(0), Buffer.alloc(1024), Buffer.alloc(1024), block(384)];
  const records = blocks.map((bytes, index) => ({ ...delivered(bytes, {
    monotonic_ns: [1000, 3500, 8000, 10500][index], native_ticks: index * 128,
  }), stream: 42, name: 'pw-record' }));
  const source = [0, 128, 256, 384].map(sourceRecord);
  const handoffs = [
    { direction: 'input', stream: 42, name: 'pw-record', dequeueMonotonicNs: 1000,
      queueMonotonicNs: 1100, ownershipDwellNs: 100 },
    { direction: 'input', stream: 42, name: 'pw-record', dequeueMonotonicNs: 3500,
      queueMonotonicNs: 3800, ownershipDwellNs: 300 },
    { direction: 'input', stream: 42, name: 'pw-record', dequeueMonotonicNs: 8000,
      queueMonotonicNs: 8500, ownershipDwellNs: 500 },
    { direction: 'input', stream: 42, name: 'pw-record', dequeueMonotonicNs: 10500,
      queueMonotonicNs: 11200, ownershipDwellNs: 700 },
  ];
  const [event] = explain([{ kind: 'invalid', sampleIndex: 128, expected: 128,
    resumedAt: 384, length: 256 }], records, source, { firstDeliveredBlock: 0 }, 0, handoffs);
  const timing = event.captureDeliveries;
  assert.equal(timing.previous.ownership.dequeueToQueueDwellNs, 100);
  assert.equal(timing.current.ownership.dequeueToQueueDwellNs, 300);
  assert.equal(timing.current.interDequeueGapNs, 2500);
  assert.equal(timing.affected.length, 2);
  assert.deepEqual(timing.affected.map(value => value.ownership.dequeueToQueueDwellNs), [300, 500]);
  assert.equal(timing.next.ownership.dequeueToQueueDwellNs, 700);
  assert.equal(timing.current.ownership.direction, 'input');
  assert.equal(timing.current.ownership.stream, 42);
  assert.equal(timing.current.ownership.name, 'pw-record');
});

test('explain does not invent ownership dwell when a handoff pair was not supplied', () => {
  const records = [delivered(block(0), { monotonic_ns: 100 }), delivered(block(128), { monotonic_ns: 200 })];
  const source = [sourceRecord(0), sourceRecord(128)];
  const [event] = explain([{ kind: 'jump', sampleIndex: 0, expected: 0, actual: 128 }],
    records, source, { firstDeliveredBlock: 0 }, 0);
  assert.equal(event.captureDeliveries.current.interDequeueGapNs, null);
  assert.equal(event.captureDeliveries.current.ownership, null);
  assert.equal(event.captureDeliveries.current.ownershipDwellNs, undefined);
  const mismatched = [{ direction: 'input', stream: 99, name: 'other-client',
    dequeueMonotonicNs: 100, queueMonotonicNs: 150, ownershipDwellNs: 50 }];
  assert.throws(() => explain([{ kind: 'jump', sampleIndex: 0, expected: 0, actual: 128 }],
    records, source, { firstDeliveredBlock: 0 }, 0, mismatched), /stream identity/);
});
