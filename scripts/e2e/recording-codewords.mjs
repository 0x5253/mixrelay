import assert from 'node:assert/strict';
import { probeSampleBits } from './probe-signal.mjs';

const key = (pcm, frame) => [0, 1, 2, 3].map(index => pcm.readUInt32LE((frame + index) * 8)).join(':');

// Four channel-zero samples identify a codeword; both channels must then match.
// A run may begin between codewords, so recover its preceding one to three frames.
export function decodeRecordingCodewords(pcm, sourceFrames) {
  assert.equal(pcm.length % 8, 0, 'Incomplete stereo Float32 frames');
  assert.ok(Number.isSafeInteger(sourceFrames) && sourceFrames >= 4);
  const source = Buffer.alloc(sourceFrames * 8), words = new Map();
  for (let frame = 0; frame < sourceFrames; frame++)
    for (let channel = 0; channel < 2; channel++) source.writeUInt32LE(probeSampleBits(frame, channel), frame * 8 + channel * 4);
  for (let frame = 0; frame + 4 <= sourceFrames; frame += 4) {
    const word = key(source, frame);
    assert.equal(words.has(word), false, 'Ambiguous source codeword');
    words.set(word, frame);
  }
  const frames = pcm.length / 8, runs = [];
  const matches = (recording, original) => original >= 0 && original < sourceFrames
    && pcm.readUInt32LE(recording * 8) === source.readUInt32LE(original * 8)
    && pcm.readUInt32LE(recording * 8 + 4) === source.readUInt32LE(original * 8 + 4);
  let nonzeroFrames = 0, nonfiniteSamples = 0;
  for (let frame = 0; frame < frames; frame++) {
    let nonzero = false;
    for (let channel = 0; channel < 2; channel++) {
      const sample = pcm.readFloatLE(frame * 8 + channel * 4);
      nonzero ||= sample !== 0;
      if (!Number.isFinite(sample)) nonfiniteSamples++;
    }
    if (nonzero) nonzeroFrames++;
  }
  for (let frame = 0; frame + 4 <= frames;) {
    // The coded source never has a zero left sample; quiet spans need no lookup key.
    if (pcm.readFloatLE(frame * 8) === 0) { frame++; continue; }
    const original = words.get(key(pcm, frame));
    if (original === undefined || ![0, 1, 2, 3].every(index => matches(frame + index, original + index))) { frame++; continue; }
    let start = frame, sourceStart = original, end = frame + 4;
    const previousEnd = runs.length ? runs.at(-1).recordingStart + runs.at(-1).frames : 0;
    while (start > previousEnd && matches(start - 1, sourceStart - 1)) { start--; sourceStart--; }
    while (end < frames && matches(end, sourceStart + end - start)) end++;
    runs.push({ recordingStart: start, sourceStart, frames: end - start });
    frame = end;
  }
  const identifiedFrames = runs.reduce((sum, run) => sum + run.frames, 0);
  return { frames, nonzeroFrames, nonfiniteSamples, identifiedFrames,
    unidentifiedNonzeroFrames: nonzeroFrames - identifiedFrames, runs };
}
