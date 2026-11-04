import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument, RuntimeGraphNode } from '../../core/domain/control/graph';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { addRoutingNode } from './routingNodeCreation';
import { updateRoutingNodeGain, updateRoutingNodeParameter } from './routingNodeParameters';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';

const stereoPorts: ProviderPorts = {
  layoutId: 'layout-v1:stereo',
  inputs: [
    {
      id: 'audio',
      name: 'Main Input',
      kind: 'main',
      channels: 2,
      channelFormat: 'stereo',
    },
  ],
  outputs: [
    {
      id: 'audio',
      name: 'Main Output',
      kind: 'main',
      channels: 2,
      channelFormat: 'stereo',
    },
  ],
};

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
const position = { x: 40, y: 60 };
const ports: ProviderPorts = {
  layoutId: 'stereo',
  inputs: [{ id: 'input', name: 'Input', kind: 'main', channels: 2, channelFormat: 'stereo' }],
  outputs: [{ id: 'output', name: 'Output', kind: 'main', channels: 2, channelFormat: 'stereo' }],
};
const processor: ExternalProcessorDescriptor = {
  displayName: 'Effect',
  typeId: 'effect',
  format: 'vst3',
  hasEditor: false,
  layouts: [ports],
  defaultLayoutId: ports.layoutId,
  parameters: [],
  vendor: 'Fixture',
  version: '1',
};
function documentWith(...nodes: RuntimeGraphNode[]): RuntimeGraphDocument {
  return { ...empty, nodes };
}
function node(model: AudioGraphModel): AudioGraphNode {
  const first = model.nodes[0];
  if (!first) throw new Error('Expected a materialized fixture node');
  return first;
}

it('materializes an external processor and persists normalized parameter changes', () => {
  const processor: ExternalProcessorDescriptor = {
    displayName: 'Fixture Compressor',
    format: 'vst3',
    hasEditor: true,
    layouts: [stereoPorts],
    defaultLayoutId: stereoPorts.layoutId,
    parameters: [
      {
        canAutomate: true,
        defaultValue: 0.5,
        id: 42,
        isReadOnly: false,
        name: 'Threshold',
        stepCount: 100,
      },
    ],
    typeId: 'vst3:00112233445566778899AABBCCDDEEFF',
    vendor: 'MixRelay Test',
    version: '1.0',
  };
  const empty: AudioGraphModel = { edges: [], nodes: [], revision: 1 };
  const added = addRoutingNode(
    empty,
    { kind: 'provider', processor, ports: stereoPorts },
    { x: 40, y: 80 },
  );
  const node = added.nodes[0];
  expect(node?.appearance.title).toBe('Fixture Compressor');
  expect(node?.appearance.meterBanks).toEqual([
    {
      channels: 2,
      label: 'Main · Main Output',
      nodeId: node?.id,
      portId: 'audio',
    },
  ]);
  expect(node?.inputPorts).toHaveLength(2);
  expect(node?.outputPorts).toHaveLength(2);
  expect(node?.appearance.parameterControls?.[0]).toMatchObject({
    id: 'external:42',
    value: 0.5,
  });

  const changed = updateRoutingNodeParameter(added, node?.id ?? '', 'external:42', 0.73);
  const serialized = audioGraphToRuntimeGraph(changed, {
    edges: [],
    nodes: [],
    revision: 1,
    schemaVersion: 2,
  });
  expect(serialized.nodes[0]?.config).toMatchObject({
    parameters: { parameterValues: { '42': 0.73 }, version: 1 },
    typeId: processor.typeId,
  });

  const restored = runtimeGraphToAudioGraph(serialized, undefined, [processor]);
  expect(restored.nodes[0]?.appearance.meterBanks).toEqual(node?.appearance.meterBanks);
  expect(restored.nodes[0]?.appearance.parameterControls?.[0]?.value).toBe(0.73);
  expect(restored.nodes[0]?.runtime?.config).toMatchObject({
    typeId: processor.typeId,
    ports: stereoPorts,
    parameters: { parameterValues: { '42': 0.73 }, version: 1 },
  });
});

it.each([Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY])(
  'does not persist a non-finite gain or processor parameter: %s',
  (value) => {
    const model = addRoutingNode(runtimeGraphToAudioGraph(empty), { kind: 'limiter' }, position);
    expect(updateRoutingNodeGain(model, node(model).id, value)).toBe(model);
    expect(updateRoutingNodeParameter(model, node(model).id, 'ceilingDb', value)).toBe(model);
  },
);

it('clamps gain at the supported bounds and keeps unchanged commands idempotent', () => {
  const model = runtimeGraphToAudioGraph(
    documentWith({
      id: 'gain',
      type: 'gain',
      label: null,
      config: { gainDb: 0, channels: 2 },
    }),
  );
  const limited = updateRoutingNodeGain(model, 'gain', 100);
  expect(node(limited).appearance.gainControl?.valueDb).toBe(12);
  expect(updateRoutingNodeGain(limited, 'gain', 12)).toBe(limited);
  const quiet = updateRoutingNodeGain(limited, 'gain', -100);
  expect(node(quiet).runtime?.config).toMatchObject({ gainDb: -60 });
  expect(updateRoutingNodeGain(quiet, 'missing', 0)).toBe(quiet);
});

it('does not write gain or parameters into an invalid config object', () => {
  const model = runtimeGraphToAudioGraph(
    documentWith({ id: 'gain', type: 'gain', label: null, config: null }),
  );
  expect(updateRoutingNodeGain(model, 'gain', 4)).toBe(model);
  expect(updateRoutingNodeParameter(model, 'missing', 'external:1', 0.2)).toBe(model);
});

it('can edit a normalized external parameter even when the stored values object is missing', () => {
  const descriptor = {
    ...processor,
    parameters: [
      {
        canAutomate: true,
        defaultValue: 0.5,
        id: 1,
        isReadOnly: false,
        name: 'Amount',
        stepCount: 100,
      },
    ],
  };
  const original = documentWith({
    id: 'effect',
    type: 'provider',
    label: null,
    config: { typeId: descriptor.typeId, ports, parameters: null },
  });
  const model = runtimeGraphToAudioGraph(original, undefined, [descriptor]);
  const changed = updateRoutingNodeParameter(model, 'effect', 'external:1', 0.7);
  expect(audioGraphToRuntimeGraph(changed, original).nodes[0]?.config).toMatchObject({
    parameters: { parameterValues: { '1': expect.closeTo(0.7, 12) } },
  });
  expect(updateRoutingNodeParameter(changed, 'effect', 'external:1', 0.7)).toBe(changed);
});

it('keeps generic parameter controls editable without inventing a built-in description', () => {
  const created = addRoutingNode(runtimeGraphToAudioGraph(empty), { kind: 'compressor' }, position);
  const first = node(created);
  if (!first.runtime) throw new Error('fixture runtime');
  const generic = {
    ...created,
    nodes: [{ ...first, runtime: { ...first.runtime, type: 'custom_processor' } }],
  };
  const changed = updateRoutingNodeParameter(generic, first.id, 'thresholdDb', -12);
  expect(node(changed).runtime?.config).toMatchObject({ thresholdDb: -12 });
  expect(node(changed).appearance.detail).toBeUndefined();
});
