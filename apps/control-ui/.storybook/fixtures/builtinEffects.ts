import { StorybookMockControlPort } from '../../src/core/adapters/fake/StorybookMockControlPort';
import { type AudioGraphModel, graphNodeDefaultWidth } from '../../src/core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../src/core/domain/control/graph';
import type { MeterSnapshot } from '../../src/core/domain/control/meters';
import {
  type EffectKind,
  effectKinds,
  effectPresets,
} from '../../src/features/routing-workspace/builtinEffects';
import {
  addRoutingNode,
  audioGraphToRuntimeGraph,
} from '../../src/features/routing-workspace/model';

export type BuiltinEffectFixtureKind = EffectKind | 'compressor' | 'limiter';
export const builtinEffectFixtureKinds: readonly BuiltinEffectFixtureKind[] = [
  'compressor',
  'limiter',
  ...effectKinds,
];

/** Fixtures contain only engine documents/layout/telemetry; presentation stays in the Feature. */
export function builtinEffectsFixture({
  kinds = builtinEffectFixtureKinds,
  preset,
  height,
  bypass = false,
  telemetry = true,
}: {
  kinds?: readonly BuiltinEffectFixtureKind[];
  preset?: string;
  height?: number;
  bypass?: boolean;
  telemetry?: boolean;
} = {}) {
  let model: AudioGraphModel = { nodes: [], edges: [], revision: 1 };
  kinds.forEach((kind, index) => {
    model = addRoutingNode(
      model,
      kind === 'compressor' || kind === 'limiter'
        ? { kind }
        : {
            kind: 'effect',
            effect: kind,
            parameters: effectPresets[kind]?.find((candidate) => candidate.label === preset)
              ?.parameters,
          },
      // Two wide dynamics cards, then three columns of the remaining effect faces.
      index < 2 && kinds.length > 2
        ? { x: index * 460, y: 0 }
        : {
            x: kinds.length > 2 ? ((index - 2) % 3) * 360 : index * 460,
            y: kinds.length > 2 ? 300 + Math.floor((index - 2) / 3) * 260 : 0,
          },
    );
  });
  const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
  const document = audioGraphToRuntimeGraph(model, empty);
  document.nodes = document.nodes.map((node) => ({ ...node, label: null }));
  if (bypass)
    document.nodes = document.nodes.map((node) => ({
      ...node,
      config: { ...(node.config as object), bypass: true },
    }));
  const meterNodes: MeterSnapshot['nodes'] = telemetry
    ? document.nodes.flatMap((node) => {
        // Fixed engine-shaped snapshots, never a second browser DSP implementation.
        if (node.type === 'compressor')
          return [
            {
              nodeId: node.id,
              portId: 'audio',
              peaks: [0.21135116, 0.10567558],
              dynamics: { inputPeaks: [1, 0.5], gainReductionDb: 13.5, thresholdDb: -18 },
            },
          ];
        if (node.type === 'limiter')
          return [
            {
              nodeId: node.id,
              portId: 'audio',
              peaks: [0.89125, 0.70557],
              dynamics: { inputPeaks: [1.2, 0.95], gainReductionDb: 2.5836, thresholdDb: -1 },
            },
          ];
        return [];
      })
    : [];
  const port = new StorybookMockControlPort({
    graph: document,
    graphStatus: {
      protocolVersion: 1,
      activeRevision: document.revision,
      submittedRevision: document.revision,
    },
    layout: {
      positions: Object.fromEntries(model.nodes.map((node) => [node.id, node.position])),
      ...(height
        ? {
            sizes: Object.fromEntries(
              model.nodes.map((node) => [node.id, { width: graphNodeDefaultWidth(node), height }]),
            ),
          }
        : {}),
      viewport: { x: 32, y: 32, zoom: 1 },
    },
    meters: { protocolVersion: 1, revision: document.revision, nodes: meterNodes },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
  const readMeters = port.getMeters.bind(port);
  port.getMeters = async () => {
    const snapshot = await readMeters();
    const status = await port.getGraphStatus();
    return snapshot.revision === status.activeRevision
      ? snapshot
      : { protocolVersion: 1, revision: status.activeRevision, nodes: [] };
  };
  return port;
}
