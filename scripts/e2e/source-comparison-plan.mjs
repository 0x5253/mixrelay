import assert from 'node:assert/strict';
import { probeSampleBits } from './probe-signal.mjs';

export const sourceOrders = ['silent-audible', 'audible-silent'];

export function sourceComparisonPlan(order) {
  assert.ok(sourceOrders.includes(order), 'Unsupported source comparison order');
  const rate = 48000, lead = 4800, duration = rate * 3, tail = rate * 1.5;
  return { order, rate, channels: 2, bufferFrames: 512, frames: lead + duration * 2 + tail,
    segments: order.split('-').map((kind, index) => ({ kind,
      start: lead + index * duration, end: lead + (index + 1) * duration })) };
}

export function sourceComparisonPcm(plan) {
  const pcm = Buffer.alloc(plan.frames * 8);
  for (const segment of plan.segments) {
    if (segment.kind !== 'audible') continue;
    for (let frame = segment.start; frame < segment.end; frame++)
      for (let channel = 0; channel < 2; channel++) pcm.writeUInt32LE(probeSampleBits(frame, channel), frame * 8 + channel * 4);
  }
  return pcm;
}
