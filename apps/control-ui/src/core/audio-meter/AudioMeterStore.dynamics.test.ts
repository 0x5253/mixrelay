import { expect, it, vi } from 'vitest';
import type { NodeMeterSnapshot } from '../domain/control/meters';
import { AudioMeterStore, audioMeterKey } from './AudioMeterStore';

const node: NodeMeterSnapshot = {
  nodeId: 'comp',
  portId: 'audio',
  peaks: [0.8, 0.4],
  dynamics: { inputPeaks: [1, 0.5], gainReductionDb: 13.5, thresholdDb: -18 },
};
const snapshot = (value: NodeMeterSnapshot) => ({
  protocolVersion: 1 as const,
  revision: 1,
  nodes: [value],
});
it('publishes a coherent dynamics reading once, preserves identical snapshots, and changes on actual gain independent of peaks', () => {
  const store = new AudioMeterStore();
  const key = audioMeterKey('comp', 'audio');
  const listener = vi.fn(() => {
    expect(store.dynamics(key)?.outputPeaks).toEqual(node.peaks);
  });
  store.subscribe(key, listener);
  expect(store.dynamics(key)).toBeNull();
  store.update(snapshot(node));
  const reading = store.dynamics(key);
  store.update(
    snapshot({
      ...node,
      peaks: [...node.peaks],
      dynamics: node.dynamics && { ...node.dynamics, inputPeaks: [1, 0.5] },
    }),
  );
  expect(store.dynamics(key)).toBe(reading);
  expect(listener).toHaveBeenCalledOnce();
  store.update(
    snapshot({ ...node, dynamics: { inputPeaks: [1, 0.5], gainReductionDb: 4, thresholdDb: -18 } }),
  );
  expect(store.dynamics(key)?.gainReductionDb).toBe(4);
  expect(listener).toHaveBeenCalledTimes(2);
});
it('tracks channel layouts and exact input/output peaks even within the legacy percent tolerance', () => {
  const store = new AudioMeterStore();
  const key = audioMeterKey('comp', 'audio');
  store.update(snapshot(node));
  const listener = vi.fn();
  store.subscribe(key, listener);
  for (const value of [
    { ...node, dynamics: { inputPeaks: [0.999, 0.5], gainReductionDb: 13.5, thresholdDb: -18 } },
    { ...node, peaks: [0.80001, 0.4] },
    {
      ...node,
      peaks: [0.8],
      dynamics: { inputPeaks: [1], gainReductionDb: 13.5, thresholdDb: -18 },
    },
    { ...node, dynamics: { inputPeaks: [1, 0.5], gainReductionDb: 13.5, thresholdDb: -24 } },
  ])
    store.update(snapshot(value));
  expect(listener).toHaveBeenCalledTimes(4);
  expect(store.dynamics(key)?.thresholdDb).toBe(-24);
});
it.each([
  { gainReductionDb: Number.NaN },
  { gainReductionDb: Number.POSITIVE_INFINITY },
  { gainReductionDb: -1 },
  { thresholdDb: Number.NaN },
  { thresholdDb: Number.POSITIVE_INFINITY },
  { inputPeaks: [] },
])('withdraws malformed dynamics telemetry instead of retaining old gain: %j', (invalid) => {
  const store = new AudioMeterStore();
  const key = audioMeterKey('comp', 'audio');
  store.update(snapshot(node));
  store.update(
    snapshot({
      ...node,
      dynamics: { inputPeaks: [1, 0.5], gainReductionDb: 13.5, thresholdDb: -18, ...invalid },
    }),
  );
  expect(store.dynamics(key)).toBeNull();
});
it('clears absent/empty/removed telemetry and sanitizes nonfinite audio peaks without inferring gain', () => {
  const store = new AudioMeterStore();
  const key = audioMeterKey('comp', 'audio');
  store.update(
    snapshot({
      ...node,
      peaks: [Number.NaN, Number.POSITIVE_INFINITY, -1, 0],
      dynamics: {
        inputPeaks: [Number.NaN, Number.POSITIVE_INFINITY, -1, 0],
        gainReductionDb: 6,
        thresholdDb: -18,
      },
    }),
  );
  expect(store.dynamics(key)).toEqual({
    inputPeaks: [0, 0, 0, 0],
    outputPeaks: [0, 0, 0, 0],
    gainReductionDb: 6,
    thresholdDb: -18,
  });
  store.update(snapshot({ ...node, peaks: [] }));
  expect(store.dynamics(key)).toBeNull();
  store.update(snapshot(node));
  store.update(snapshot({ ...node, dynamics: undefined }));
  expect(store.dynamics(key)).toBeNull();
  store.update(snapshot(node));
  store.update({ protocolVersion: 1, revision: 2, nodes: [] });
  expect(store.dynamics(key)).toBeNull();
  store.update(snapshot(node));
  const removed = vi.fn();
  const unsubscribe = store.subscribe(key, removed);
  unsubscribe();
  store.clear();
  expect(store.dynamics(key)).toBeNull();
  expect(removed).not.toHaveBeenCalled();
});
