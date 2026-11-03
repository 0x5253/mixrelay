import { expect, it } from 'vitest';
import { type ControlStateSnapshot, graphControls } from './state';

const snapshot: ControlStateSnapshot = {
  generation: 1,
  status: {
    protocolVersion: 1,
    driverState: 'ready',
    sampleRate: 48000,
    bufferFrames: 128,
    underruns: 0,
    overruns: 0,
  },
  graph: { protocolVersion: 1, activeRevision: 2, submittedRevision: 2 },
  controls: {
    revision: 2,
    activeRevision: 2,
    structureGeneration: 1,
    graphs: [{ id: 'a', nodes: [] }],
  },
  editors: [],
};

it('reads one graph’s node controls at the snapshot’s control revision', () => {
  expect(graphControls(snapshot, 'a')).toEqual({ graphId: 'a', revision: 2, nodes: [] });
  expect(graphControls(snapshot, 'missing')).toBeUndefined();
  expect(graphControls({ ...snapshot, controls: null }, 'a')).toBeUndefined();
  expect(graphControls(undefined, 'a')).toBeUndefined();
});
