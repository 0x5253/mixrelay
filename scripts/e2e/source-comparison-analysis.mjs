import assert from 'node:assert/strict';

const integer = value => Number.isSafeInteger(value) && value >= 0;

function timingRows(timing) {
  const base = timing.split('\n').find(line => line.startsWith('timebase,'))?.split(',').slice(1).map(Number);
  assert.ok(base?.length === 2 && base.every(value => integer(value) && value > 0), 'Missing native host timebase');
  return timing.split('\n').filter(line => /^(input|output),/.test(line)).map(line => {
    const [side, ...fields] = line.split(',');
    const numbers = fields.map(Number);
    assert.ok(numbers.length === 7 && numbers.every(integer), 'Invalid source comparison timing row');
    const [first, sample, host, frames, flags, entered, exited] = numbers;
    assert.ok(frames > 0 && (flags & 3) === 3 && exited >= entered, 'Invalid native callback timing');
    return { side, first, sample, host, frames, entered, exited };
  });
}

function coverage(rows, origin, start, end) {
  const seen = new Uint8Array(end - start);
  let duplicateFrames = 0;
  for (const row of rows) {
    const first = Math.max(start, row.sample - origin), last = Math.min(end, row.sample - origin + row.frames);
    for (let frame = first; frame < last; frame++) {
      if (seen[frame - start]) duplicateFrames++;
      seen[frame - start] = 1;
    }
  }
  return { frames: seen.length, missingFrames: seen.reduce((sum, present) => sum + Number(!present), 0), duplicateFrames };
}

// Silent payloads cannot identify repeated samples; native coordinates are checked separately.
export function analyseSourceComparison(plan, source, metrics, timing, capture) {
  assert.equal(source.length, plan.frames * 8);
  assert.equal(metrics.sourceFrames, plan.frames);
  assert.equal(capture.length, metrics.recordedFrames * 8, 'Incomplete source comparison raw capture');
  assert.ok(integer(metrics.origin) && metrics.hasOrigin === true);
  const rows = timingRows(timing), inputs = rows.filter(row => row.side === 'input'), outputs = rows.filter(row => row.side === 'output');
  assert.equal(inputs.length, metrics.inputCallbacks);
  assert.equal(outputs.length, metrics.outputCallbacks);
  let cursor = 0;
  for (const row of inputs) {
    assert.equal(row.first, cursor, 'Captured chunks must account for every raw frame');
    cursor += row.frames;
  }
  assert.equal(cursor, metrics.recordedFrames);
  for (const row of outputs) assert.equal(row.sample - row.first, metrics.origin, 'Output source origin changed');
  const nativeErrors = [];
  for (const [key, expected] of Object.entries({ rate: plan.rate, channels: 2, bufferFrames: 512,
    invalidBuffers: 0, invalidTimestamps: 0, overflow: 0, droppedChunks: 0,
    activityProtection: true, completed: true, captureSaved: true, traceSaved: true, traceDisabled: true }))
    if (metrics[key] !== expected) nativeErrors.push({ key, expected, observed: metrics[key] ?? null });
  for (const key of ['create', 'start', 'stop', 'destroy'])
    if (metrics.nativeStatus?.[key] !== 0) nativeErrors.push({ key: `nativeStatus.${key}`, observed: metrics.nativeStatus?.[key] ?? null });
  const consent = metrics.inputAuthorization;
  if (consent?.initial !== 3 || consent?.final !== 3 || consent?.requested !== false)
    nativeErrors.push({ key: 'inputAuthorization', observed: consent ?? null });
  if (rows.some(row => row.frames !== plan.bufferFrames)) nativeErrors.push({ key: 'callbackFrameCount' });
  const discontinuities = {};
  for (const side of ['input', 'output']) {
    const selected = rows.filter(row => row.side === side);
    discontinuities[side] = selected.slice(1).flatMap((row, index) => {
      const expected = selected[index].sample + selected[index].frames;
      const faults = row.sample === expected ? [] : [{ kind: 'sample', expected, observed: row.sample, frames: row.sample - expected }];
      if (row.host <= selected[index].host) faults.push({ kind: 'host', previous: selected[index].host, observed: row.host });
      return faults;
    });
  }
  const segments = plan.segments.map(segment => {
    const result = { ...segment, input: coverage(inputs, metrics.origin, segment.start, segment.end),
      output: coverage(outputs, metrics.origin, segment.start, segment.end),
      wrongFrames: 0, nonfiniteSamples: 0, zeroFrames: 0, differences: [],
      sampleIdentityObservable: segment.kind === 'audible' };
    for (const row of inputs) {
      let difference;
      for (let offset = 0; offset < row.frames; offset++) {
        const frame = row.sample - metrics.origin + offset;
        if (frame < segment.start || frame >= segment.end) continue;
        let wrong = false, zero = true;
        for (let channel = 0; channel < 2; channel++) {
          const index = (row.first + offset) * 8 + channel * 4;
          const actual = capture.readUInt32LE(index), expected = source.readUInt32LE(frame * 8 + channel * 4);
          const digitalZero = (actual & 0x7fffffff) === 0;
          wrong ||= expected === 0 ? !digitalZero : actual !== expected;
          zero &&= digitalZero;
          result.nonfiniteSamples += Number(!Number.isFinite(capture.readFloatLE(index)));
        }
        result.zeroFrames += Number(zero);
        if (!wrong) { difference = null; continue; }
        result.wrongFrames++;
        if (!difference || difference.end !== frame) {
          difference = { start: frame, end: frame, recordedStart: row.first + offset };
          result.differences.push(difference);
        }
        difference.end++;
      }
    }
    const period = metrics.timestampPeriod;
    if (Number.isSafeInteger(period) && period > 0) for (const difference of result.differences) {
      const earlierStart = difference.start - period, earlierEnd = difference.end - period;
      if (!plan.segments.some(segment => segment.kind === 'audible' && earlierStart >= segment.start && earlierEnd <= segment.end)) continue;
      let matches = true;
      for (let frame = 0; frame < difference.end - difference.start && matches; frame++) for (let channel = 0; channel < 2; channel++)
        matches &&= capture.readUInt32LE((difference.recordedStart + frame) * 8 + channel * 4)
          === source.readUInt32LE((earlierStart + frame) * 8 + channel * 4);
      difference.matchesPreviousTimestampPeriod = matches;
    }
    result.passed = [result.input, result.output].every(side => side.missingFrames === 0 && side.duplicateFrames === 0)
      && result.wrongFrames === 0 && result.nonfiniteSamples === 0;
    return result;
  });
  return { order: plan.order, origin: metrics.origin, nativeErrors, discontinuities, segments,
    passed: nativeErrors.length === 0 && Object.values(discontinuities).every(rows => rows.length === 0) && segments.every(segment => segment.passed),
    limitations: ['Digital silence verifies zero payload and callback timeline coverage; it cannot identify repeated or stale zero samples.',
      'This diagnostic does not replace the required AUDIO-01 or REC-02 verdict.'] };
}
