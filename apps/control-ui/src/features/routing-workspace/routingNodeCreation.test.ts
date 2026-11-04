import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { addRoutingNode } from './routingNodeCreation';
import { updateRoutingNodeParameter } from './routingNodeParameters';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import { virtualPolicy, virtualSpec } from './virtualEndpointFixtures';

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

const spec = {
  ...virtualSpec,
  displayName: 'Endpoint name',
  render: { channelLayout: 'stereo' as const, channels: 2 },
  capture: { channelLayout: 'stereo' as const, channels: 2 },
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
function node(model: AudioGraphModel): AudioGraphNode {
  const first = model.nodes[0];
  if (!first) throw new Error('Expected a materialized fixture node');
  return first;
}

it.each(['virtual_input', 'virtual_output'] as const)(
  'recreates %s from an empty graph with the required endpoint identity',
  (kind) => {
    const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
    const model = addRoutingNode(runtimeGraphToAudioGraph(empty), { kind, spec }, { x: 40, y: 80 });
    const serialized = audioGraphToRuntimeGraph(model, empty);
    expect(serialized.nodes).toEqual([
      {
        id: expect.stringMatching(/^virtual-[0-9a-f-]{36}$/),
        label: spec.displayName,
        type: 'virtual_endpoint',
        config: {
          ...spec,
          gainDb: 0,
          reference: {
            render: kind === 'virtual_input',
            capture: kind === 'virtual_output',
          },
        },
      },
    ]);
    const both = addRoutingNode(
      runtimeGraphToAudioGraph(serialized),
      {
        kind: kind === 'virtual_input' ? 'virtual_output' : 'virtual_input',
        spec,
      },
      { x: 400, y: 80 },
    );
    const reunited = audioGraphToRuntimeGraph(both, serialized);
    expect(reunited.nodes).toHaveLength(1);
    expect(reunited.nodes[0]?.config).toMatchObject({
      id: spec.id,
      render: { channels: 2 },
      capture: { channels: 2 },
    });
  },
);

it('persists and rehydrates the exact selected multibus geometry, independent of catalog default', () => {
  const selectedPorts: ProviderPorts = {
    layoutId: 'layout-v1:surround-sidechain',
    inputs: [
      {
        id: 'audio',
        name: 'Main',
        kind: 'main',
        channels: 8,
        channelFormat: 'surround7_1',
      },
      {
        id: 'bus:1',
        name: 'Detector',
        kind: 'sidechain',
        channels: 2,
        channelFormat: 'stereo',
      },
    ],
    outputs: [
      {
        id: 'audio',
        name: 'Main',
        kind: 'main',
        channels: 6,
        channelFormat: 'surround5_1',
      },
      {
        id: 'bus:1',
        name: 'Auxiliary',
        kind: 'auxiliary',
        channels: 3,
        channelFormat: 'discrete',
      },
    ],
  };
  const processor: ExternalProcessorDescriptor = {
    displayName: 'Multibus Effect',
    format: 'clap',
    hasEditor: false,
    layouts: [stereoPorts, selectedPorts],
    defaultLayoutId: stereoPorts.layoutId,
    parameters: [],
    typeId: 'clap:multibus',
    vendor: 'MixRelay Test',
    version: '1',
  };
  const empty: RuntimeGraphDocument = {
    edges: [],
    nodes: [],
    revision: 1,
    schemaVersion: 2,
  };
  const added = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'provider', processor, ports: selectedPorts },
    { x: 50, y: 90 },
  );
  const selectedNode = added.nodes[0];
  expect(selectedNode?.inputPorts.map((port) => port.runtimePortId)).toEqual([
    ...Array(8).fill('audio'),
    ...Array(2).fill('bus:1'),
  ]);
  expect(selectedNode?.outputPorts.map((port) => port.runtimePortId)).toEqual([
    ...Array(6).fill('audio'),
    ...Array(3).fill('bus:1'),
  ]);
  expect(selectedNode?.inputPorts[8]?.label).toBe('Sidechain · Detector 1');
  expect(selectedNode?.outputPorts[6]?.label).toBe('Auxiliary · Auxiliary 1');
  expect(selectedNode?.appearance.channelCount).toBe(9);

  const saved = audioGraphToRuntimeGraph(added, empty);
  expect(saved.nodes[0]?.config).toMatchObject({ typeId: processor.typeId, ports: selectedPorts });
  const rehydrated = runtimeGraphToAudioGraph(saved, undefined, [processor]);
  expect(rehydrated.nodes[0]?.inputPorts).toEqual(selectedNode?.inputPorts);
  expect(rehydrated.nodes[0]?.outputPorts).toEqual(selectedNode?.outputPorts);
  expect(rehydrated.nodes[0]?.appearance.offline).toBeUndefined();

  const offline = runtimeGraphToAudioGraph(saved);
  expect(offline.nodes[0]?.inputPorts).toEqual(selectedNode?.inputPorts);
  expect(offline.nodes[0]?.outputPorts).toEqual(selectedNode?.outputPorts);
  expect(offline.nodes[0]?.runtime?.config).toEqual(saved.nodes[0]?.config);
});

it.each(['virtual_input', 'virtual_output'] as const)(
  'new %s identities survive deletion without being reassigned to another device',
  (kind) => {
    const empty: AudioGraphModel = { nodes: [], edges: [], revision: 1 };
    const first = addRoutingNode(empty, { kind, spec }, { x: 0, y: 0 });
    const firstId = first.nodes[0]?.runtime?.binding?.nodeId;
    const deleted = { ...first, nodes: [] };
    const replacement = addRoutingNode(
      deleted,
      { kind, spec: { ...spec, id: 'device-b' } },
      { x: 0, y: 0 },
    );
    const replacementId = replacement.nodes[0]?.runtime?.binding?.nodeId;
    expect(replacementId).not.toBe(firstId);
    expect(replacementId).toMatch(/^virtual-[0-9a-f-]{36}$/);
    const both = addRoutingNode(
      replacement,
      {
        kind: kind === 'virtual_input' ? 'virtual_output' : 'virtual_input',
        spec: { ...spec, id: 'device-b' },
      },
      { x: 400, y: 0 },
    );
    expect(both.nodes).toHaveLength(2);
    expect(both.nodes.map((node) => node.runtime?.binding?.nodeId)).toEqual([
      replacementId,
      replacementId,
    ]);
  },
);

it.each(['limiter', 'compressor'] as const)(
  'creates and round-trips the built-in %s without external-plugin metadata',
  (kind) => {
    const created = addRoutingNode(runtimeGraphToAudioGraph(empty), { kind }, position);
    const first = node(created);
    const parameter = kind === 'limiter' ? 'ceilingDb' : 'thresholdDb';
    const changed = updateRoutingNodeParameter(created, first.id, parameter, -9.2);
    const serialized = audioGraphToRuntimeGraph(changed, empty);
    expect(serialized.nodes[0]?.config).toMatchObject({ [parameter]: -9, channels: 2 });
    const restored = runtimeGraphToAudioGraph({
      ...serialized,
      nodes: serialized.nodes.map((saved) => ({ ...saved, label: null })),
    });
    expect(node(restored).appearance.title).toBe(kind === 'limiter' ? 'Limiter' : 'Compressor');
    expect(
      node(restored).appearance.parameterControls?.find((control) => control.id === parameter),
    ).toMatchObject({ value: -9, suffix: 'dB' });
    expect(node(restored).runtime?.type).toBe(kind);
    expect(node(restored).appearance.detail).toContain('-9.0 dB');
    expect(updateRoutingNodeParameter(changed, first.id, parameter, -9)).toBe(changed);
  },
);

it('cannot create an uninstalled virtual device when the controller disallows creation', () => {
  const initial = runtimeGraphToAudioGraph(empty);
  const result = addRoutingNode(
    initial,
    { kind: 'virtual_input', spec: virtualSpec },
    position,
    undefined,
    { ...virtualPolicy, canCreate: false, devices: [] },
  );
  expect(result).toBe(initial);
  const existing = addRoutingNode(
    initial,
    { kind: 'virtual_input', spec: virtualSpec },
    position,
    undefined,
    { ...virtualPolicy, canCreate: false },
  );
  expect(existing.nodes).toHaveLength(1);
});

it('does not materialize a side whose native endpoint has no channels for that direction', () => {
  const model = runtimeGraphToAudioGraph(empty);
  const spec = { ...virtualSpec, capture: { channels: 0, channelLayout: 'discrete' as const } };
  expect(addRoutingNode(model, { kind: 'virtual_input', spec }, position)).toBe(model);
});

it('accepts unnamed processor buses and falls back to their bus kind', () => {
  const unnamed = {
    ...ports,
    inputs: ports.inputs.map((bus) => ({ ...bus, name: '' })),
    outputs: ports.outputs.map((bus) => ({ ...bus, name: '' })),
  };
  const model = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'provider', processor, ports: unnamed },
    position,
  );
  expect(node(model).inputPorts[0]?.label).toBe('Main 1');
  expect(node(model).outputPorts[0]?.label).toBe('Main 1');
});
