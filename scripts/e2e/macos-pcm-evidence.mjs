import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { analyseHalPcmContract } from './hal-pcm-contract.mjs';
import { intersectPcmRanges, pcmRangeFrames } from './pcm-ranges.mjs';
import { probeSampleBits } from './probe-signal.mjs';
import { analyseRecordingPcmContract } from './recording-pcm-contract.mjs';

export function sourcePcmReference(plan, product) {
  return { order: plan.order, sourceFrames: plan.frames, segments: plan.segments,
    sourceRanges: product.sourceRanges, productPassed: product.passed };
}

export function requireObservedSegments(product, segments) {
  const observedSegments = segments.map(segment => ({ ...segment,
    publishedFrames: pcmRangeFrames(intersectPcmRanges(product.sourceRanges, segment.start, segment.end)),
    comparedFrames: pcmRangeFrames(intersectPcmRanges(product.comparedSourceRanges, segment.start, segment.end)) }));
  const insufficient = observedSegments.filter(segment => segment.publishedFrames < 24000 || segment.comparedFrames < 24000);
  return { ...product, observedSegments,
    evidenceIssues: [...product.evidenceIssues, ...insufficient.map(segment => ({ reason: 'insufficient_observed_input', segment }))],
    evidenceComplete: product.evidenceComplete && insufficient.length === 0,
    passed: product.passed && insufficient.length === 0 };
}

export function probePcmReference(measured, timing) {
  assert.equal(measured.sameDevice, true, 'Product PCM evidence requires one native device');
  assert.equal(measured.output.rate, 48000);
  assert.equal(measured.input.rate, measured.output.rate);
  assert.equal(measured.input.channels, 2);
  assert.equal(measured.output.channels, 2);
  const first = timing.split('\n').find(line => line.startsWith('output,'))?.split(',');
  assert.equal(first?.length, 8, 'Missing native output origin');
  const origin = Number(first[2]) - Number(first[1]);
  assert.ok(Number.isSafeInteger(origin) && origin >= 0);
  assert.ok(Number.isSafeInteger(measured.signalFrames) && measured.signalFrames >= 24000 && measured.signalFrames <= 48000 * 120);
  const lead = 4800, end = lead + measured.signalFrames, source = Buffer.alloc(end * 8);
  for (let frame = lead; frame < end; frame++) for (let channel = 0; channel < 2; channel++)
    source.writeUInt32LE(probeSampleBits(frame, channel), frame * 8 + channel * 4);
  const plan = { order: 'roundtrip', frames: end,
    segments: [{ kind: 'silent', start: 0, end: lead }, { kind: 'audible', start: lead, end }] };
  return { origin, source, plan };
}

export function readProbePcmEvidence(directory, measured) {
  const { origin, source, plan } = probePcmReference(measured, readFileSync(join(directory, 'native-probe.stderr.log'), 'utf8'));
  const trace = JSON.parse(readFileSync(join(directory, 'native-hal-io.json'), 'utf8'));
  assert.equal(readFileSync(join(directory, 'native-signal.f32')).length, measured.recordedFrames * 8, 'Incomplete native raw capture');
  const product = requireObservedSegments(analyseHalPcmContract(trace, { source, origin, channels: 2 }),
    plan.segments.filter(segment => segment.kind === 'audible'));
  return { product, reference: sourcePcmReference(plan, product) };
}

export function readProbeRecordingEvidence(directory) {
  const measured = JSON.parse(readFileSync(join(directory, 'native-probe.stdout.json'), 'utf8'));
  const { reference } = readProbePcmEvidence(directory, measured);
  return analyseRecordingPcmContract(readFileSync(join(directory, 'recording.wav')), [reference]);
}
