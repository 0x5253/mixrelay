import { expect, it } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { presentProcessorTimings } from './processorTiming';

it('isolates graph-local timings and removes missing samples without changing the audio model', () => {
  const model: AudioGraphModel = {
    revision: 5,
    edges: [],
    nodes: [
      {
        id: 'fx',
        position: { x: 0, y: 0 },
        inputPorts: [],
        outputPorts: [],
        appearance: { title: 'Effect', category: 'Processor', icon: 'AU' },
        runtime: { type: 'provider', config: {} },
      },
    ],
  };
  const samples = [
    { graphId: 'A', nodeId: 'fx', roundTripNs: 1000 },
    { graphId: 'B', nodeId: 'fx', roundTripNs: 2000 },
  ];
  const presented = presentProcessorTimings(model, samples, 'A');
  expect(presented?.nodes[0]?.appearance.processingTimeNs).toBe(1000);
  expect(presented?.revision).toBe(5);
  expect(model.nodes[0]?.appearance.processingTimeNs).toBeUndefined();
  expect(presentProcessorTimings(model, samples, 'B')?.nodes[0]?.appearance.processingTimeNs).toBe(
    2000,
  );
  expect(
    presentProcessorTimings(presented, [], 'A')?.nodes[0]?.appearance.processingTimeNs,
  ).toBeUndefined();
  expect(presentProcessorTimings(null, samples, 'A')).toBeNull();
});
