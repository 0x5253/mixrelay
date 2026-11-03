import type { AudioGraphModel } from '../../core/audio-graph/model';

/** The graph's response uses the running engine format, without changing saved DSP values. */
export function presentEqualizerSampleRate(
  model: AudioGraphModel | null,
  sampleRate: number | undefined,
) {
  if (!model) return null;
  if (!model.nodes.some((node) => node.appearance.equalizer)) return model;
  return {
    ...model,
    nodes: model.nodes.map((node) => {
      const equalizer = node.appearance.equalizer;
      if (!equalizer) return node;
      return {
        ...node,
        appearance: {
          ...node.appearance,
          equalizer: { ...equalizer, sampleRate },
        },
      };
    }),
  };
}
