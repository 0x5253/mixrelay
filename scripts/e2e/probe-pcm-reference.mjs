import assert from 'node:assert/strict';

import { probeSampleBits } from './probe-signal.mjs';
import { analyseNativeReference } from './probe-native-reference.mjs';
import { probeTimelineCoverage } from './probe-timeline-coverage.mjs';

export function analyseProbeReference(trace, metrics, timing, pcm) {
  assert.equal(metrics.sameDevice, true, 'Reference requires the same native device');
  assert.equal(metrics.output.rate, metrics.input.rate, 'Reference requires equal native rates');
  assert.equal(metrics.output.channels, metrics.input.channels);
  const channels = metrics.input.channels;
  assert.ok(Number.isSafeInteger(channels) && channels > 0 && channels <= 64);
  assert.equal(pcm.length, metrics.recordedFrames * channels * 4, 'Incomplete original Float32 capture');
  const rows = timing.split('\n').filter(line => /^(input|output),/.test(line)).map(line => {
    const [side, ...numbers] = line.split(',');
    assert.equal(numbers.length, 7, 'Unsupported original probe timing schema');
    const [first, sample, host, frames, flags, enter, exit] = numbers.map(Number);
    assert.ok([first, sample, host, frames, flags, enter, exit].every(value => Number.isSafeInteger(value) && value >= 0));
    return { side, first, sample, frames, flags };
  });
  const first = rows.find(row => row.side === 'output');
  assert.ok(first && (first.flags & 1), 'Missing valid output origin');
  const origin = first.sample - first.first;
  const start = origin + Math.trunc(metrics.output.rate * 0.1), end = start + metrics.signalFrames;
  const native = analyseNativeReference(trace, { origin, start, end, channels });
  const inputDifferences = [];
  for (const row of rows.filter(row => row.side === 'input')) {
    assert.ok((row.flags & 1) && row.first + row.frames <= metrics.recordedFrames, 'Invalid input capture coordinates');
    let mismatch = null;
    const flush = () => {
      if (mismatch) inputDifferences.push(mismatch);
      mismatch = null;
    };
    for (let frame = 0; frame < row.frames; frame++) {
      const device = row.sample + frame, recorded = row.first + frame;
      if (device < start || device >= end) { flush(); continue; }
      let wrong = false, zero = true;
      for (let channel = 0; channel < channels; channel++) {
        const bits = pcm.readUInt32LE((recorded * channels + channel) * 4);
        wrong ||= bits !== probeSampleBits(device - origin, channel);
        zero &&= (bits & 0x7fffffff) === 0;
      }
      if (!wrong) { flush(); continue; }
      mismatch ??= { deviceStart: device, deviceEnd: device, recordedStart: recorded, frames: 0, zeroFrames: 0 };
      mismatch.deviceEnd++;
      mismatch.frames++;
      mismatch.zeroFrames += Number(zero);
    }
    flush();
  }
  for (const mismatch of inputDifferences) {
    const period = metrics.input.timestampPeriod;
    if (Number.isSafeInteger(period) && period > 0 && mismatch.deviceStart - period >= start) {
      let matches = true;
      for (let frame = 0; frame < mismatch.frames && matches; frame++) {
        for (let channel = 0; channel < channels; channel++) {
          const bits = pcm.readUInt32LE(((mismatch.recordedStart + frame) * channels + channel) * 4);
          matches &&= bits === probeSampleBits(mismatch.deviceStart + frame - origin - period, channel);
        }
      }
      // A bit match records displacement only; it does not identify the host's cache implementation.
      mismatch.matchesPreviousTimestampPeriod = matches;
    }
    mismatch.retainedCaptureRows = trace.rows.filter(row => row[1] === 0
      && row[2] < mismatch.deviceEnd && row[2] + row[3] > mismatch.deviceStart);
    mismatch.retainedWriteRows = trace.rows.filter(row => row[1] === 1
      && row[2] < mismatch.deviceEnd && row[2] + row[3] > mismatch.deviceStart);
  }
  const timelineCoverage = probeTimelineCoverage(rows, trace, {
    start, end, sourceClient: native.nativeWrites.sourceClient,
  });
  return { origin, signalWindow: { start, end }, ...native, timelineCoverage, inputDifferences,
    limitations: ['Reference applies to the integer-coded default probe signal and its original timing/capture files.',
      'Native trace retention and missing epoch labels still limit attribution. This comparison does not change qualification.'] };
}
