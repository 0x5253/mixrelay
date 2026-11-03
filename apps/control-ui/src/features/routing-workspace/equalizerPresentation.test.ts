import { expect, it } from 'vitest';
import { presentEqualizerSampleRate } from './equalizerPresentation';
import { addRoutingNode, runtimeGraphToAudioGraph } from './model';

it('renders EQ at the engine sample rate while preserving authored parameters and other nodes', () => {
  const empty = runtimeGraphToAudioGraph({ schemaVersion: 2, revision: 1, nodes: [], edges: [] });
  const eq = addRoutingNode(empty, { kind: 'effect', effect: 'equalizer' }, { x: 0, y: 0 });
  const model = addRoutingNode(eq, { kind: 'compressor' }, { x: 0, y: 0 });
  expect(presentEqualizerSampleRate(empty, 96000)).toBe(empty);
  const presented = presentEqualizerSampleRate(model, 96000);
  expect(presented?.nodes[0]?.appearance.equalizer?.sampleRate).toBe(96000);
  expect(presented?.nodes[0]?.runtime).toBe(model.nodes[0]?.runtime);
  expect(presented?.nodes[1]).toBe(model.nodes[1]);
  expect(presented?.revision).toBe(model.revision);
  expect(model.nodes[0]?.appearance.equalizer?.sampleRate).toBeUndefined();
});

it('keeps an unavailable engine rate unset without changing authored DSP parameters', () => {
  const model = addRoutingNode(
    runtimeGraphToAudioGraph({ schemaVersion: 2, revision: 1, nodes: [], edges: [] }),
    { kind: 'effect', effect: 'equalizer' },
    { x: 0, y: 0 },
  );
  const eq = presentEqualizerSampleRate(model, undefined)?.nodes[0];
  expect(eq?.appearance.equalizer?.sampleRate).toBeUndefined();
  expect(presentEqualizerSampleRate(null, 48000)).toBeNull();
});
