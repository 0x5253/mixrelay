import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pcmHash } from '../pcm-hash.mjs';
import { probeSampleBits } from '../probe-signal.mjs';

// A finite native observation for process/observer tests; no audio device opens.
export function writeProbeEvidence(directory, measured, { corruptRead = true, traceVersion = 5 } = {}) {
  const frames = 30720, source = Buffer.alloc(frames * 8), rows = [], timing = ['timebase,1,1'];
  for (let frame = 4800; frame < frames; frame++) for (let channel = 0; channel < 2; channel++)
    source.writeUInt32LE(probeSampleBits(frame, channel), frame * 8 + channel * 4);
  for (let first = 0; first < frames; first += 512) {
    const pcm = source.subarray(first * 8, (first + 512) * 8);
    const hash = pcmHash(512, 2, (frame, channel) => pcm.readUInt32LE(frame * 8 + channel * 4));
    let zero = 0;
    for (let frame = 0; frame < 512; frame++) zero += Number(pcm.readFloatLE(frame * 8) === 0 && pcm.readFloatLE(frame * 8 + 4) === 0);
    for (const render of [1, 0]) rows.push([rows.length + 1, render, 1000 + first, 512, 1, 512, zero,
      !render && corruptRead && first === 5120 ? '0000000000000000' : hash, render ? 17 : 0,
      first * 10 + (render ? 1 : 3), first * 10 + (render ? 2 : 4)]);
    timing.push(`output,${first},${1000 + first},${first * 10 + 1},512,7,${first * 10 + 1},${first * 10 + 2}`);
  }
  const metrics = { ...measured, sameDevice: true, signalFrames: 144000, recordedFrames: frames,
    output: { rate: 48000, channels: 2 }, input: { rate: 48000, channels: 2 } };
  writeFileSync(join(directory, 'native-probe.stdout.json'), JSON.stringify(metrics));
  writeFileSync(join(directory, 'native-probe.stderr.log'), timing.join('\n') + '\n');
  writeFileSync(join(directory, 'native-signal.f32'), source);
  writeFileSync(join(directory, 'native-hal-io.json'), JSON.stringify({ version: traceVersion, issued: rows.length,
    dropped: 0, capacity: 8192, rows, admission: { read: rows.length / 2, write: rows.length / 2,
      timestamp: 0, extent: 0, stream: 0, frozen: 0, inactive: 0, native: 0,
      lastRejectedTimestampBits: '0000000000000000', lastRejectedExtent: { frames: 0, bufferPresent: false, render: false },
      nullBuffer: 0, oversizedFrames: 0 } }));
  return metrics;
}
