import { expect, it } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument, RuntimeGraphNode } from '../../core/domain/control/graph';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import {
  addRoutingNode,
  audioGraphToRuntimeGraph,
  defaultPresentation,
  providerPortsFrom,
  runtimeGraphToAudioGraph,
  updateRoutingNodeParameter,
} from './model';
import { duplexReference, virtualSpec } from './virtualEndpointFixtures';

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

const DEFAULT_ENDPOINT_NODE_ID = 'test-boundary';
const spec = {
  ...virtualSpec,
  displayName: 'Endpoint name',
  render: { channelLayout: 'stereo' as const, channels: 2 },
  capture: { channelLayout: 'stereo' as const, channels: 2 },
};

function endpoint(): RuntimeGraphNode {
  return {
    config: {
      ...spec,
      reference: duplexReference,
      capture: { channelLayout: 'stereo', channels: 2 },
      render: { channelLayout: 'stereo', channels: 2 },
    },
    id: DEFAULT_ENDPOINT_NODE_ID,
    label: null,
    type: 'virtual_endpoint',
  };
}

function directDocument(): RuntimeGraphDocument {
  return {
    edges: [
      {
        channelMapping: { mode: 'auto' },
        destination: { nodeId: DEFAULT_ENDPOINT_NODE_ID, portId: 'to_os' },
        id: 'direct',
        source: { nodeId: DEFAULT_ENDPOINT_NODE_ID, portId: 'from_os' },
      },
    ],
    nodes: [endpoint()],
    revision: 4,
    schemaVersion: 2,
  };
}

function gainFixture(): RuntimeGraphDocument {
  return {
    ...directDocument(),
    nodes: [
      endpoint(),
      {
        id: 'output-gain',
        label: 'User gain label',
        type: 'gain',
        config: { channels: 2, gainDb: -6.0206003 },
      },
    ],
    edges: [
      {
        id: 'gain-in',
        source: { nodeId: DEFAULT_ENDPOINT_NODE_ID, portId: 'from_os' },
        destination: { nodeId: 'output-gain', portId: 'audio' },
        channelMapping: { mode: 'auto' },
      },
      {
        id: 'gain-out',
        source: { nodeId: 'output-gain', portId: 'audio' },
        destination: { nodeId: DEFAULT_ENDPOINT_NODE_ID, portId: 'to_os' },
        channelMapping: { mode: 'auto' },
      },
    ],
  };
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

it('maps the duplex OS boundary and gain node without inventing runtime configuration', () => {
  const gainDocument = gainFixture();
  const model = runtimeGraphToAudioGraph(gainDocument);

  expect(model.nodes[0]?.appearance.title).toBe(spec.displayName);
  expect(model.nodes[0]?.outputPorts[0]?.boundary).toBe('source');
  expect(model.nodes[1]?.inputPorts[0]?.boundary).toBe('sink');
  expect(model.nodes[2]?.appearance.detail).toBe('-6.0 dB');
  expect(model.nodes[0]?.outputPorts).toHaveLength(2);
  expect(model.nodes[1]?.inputPorts).toHaveLength(2);
  expect(model.nodes[0]?.appearance.meterBanks).toEqual([
    { channels: 2, nodeId: DEFAULT_ENDPOINT_NODE_ID, portId: 'from_os' },
  ]);
  expect(model.nodes[1]?.appearance.meterBanks).toEqual([
    { channels: 2, nodeId: DEFAULT_ENDPOINT_NODE_ID, portId: 'to_os' },
  ]);
  expect(model.edges).toHaveLength(4);

  const restored = audioGraphToRuntimeGraph(model, gainDocument);
  expect(restored.nodes).toEqual(gainDocument.nodes);
  expect(restored.edges).toEqual(gainDocument.edges);
});

it('preserves user labels even for IDs used by former demonstration fixtures', () => {
  const original = gainFixture();
  const model = runtimeGraphToAudioGraph(original);
  expect(model.nodes.find((node) => node.id === 'output-gain')?.appearance.title).toBe(
    'User gain label',
  );
  expect(audioGraphToRuntimeGraph(model, original).nodes).toEqual(original.nodes);
});

it('renders unknown runtime nodes safely and drops UI-only nodes on serialization', () => {
  const document: RuntimeGraphDocument = {
    ...directDocument(),
    nodes: [
      { config: { gainDb: 'invalid' }, id: 'bad-gain', label: null, type: 'gain' },
      { config: null, id: 'empty-gain', label: 'Empty', type: 'gain' },
      { config: {}, id: 'future', label: null, type: 'future_node' },
    ],
  };
  const model = runtimeGraphToAudioGraph(document);
  expect(model.nodes[0]?.appearance.category).toBe('Unsupported');
  expect(model.nodes[1]?.appearance.category).toBe('Unsupported');
  expect(model.nodes[2]?.appearance.category).toBe('Unsupported');

  const withUiOnlyNode: AudioGraphModel = {
    ...model,
    nodes: [
      ...model.nodes,
      {
        appearance: { category: 'Test', detail: '', icon: '?', title: 'UI only' },
        id: 'ui-only',
        inputPorts: [],
        outputPorts: [],
        position: { x: 0, y: 0 },
      },
    ],
  };
  expect(audioGraphToRuntimeGraph(withUiOnlyNode, document).nodes).toHaveLength(3);
});

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

it('keeps authored layout geometry and marks it unavailable when the installed layout changes', () => {
  const authoredPorts: ProviderPorts = {
    layoutId: 'layout-v1:selected',
    inputs: [
      {
        id: 'audio',
        name: 'Main',
        kind: 'main',
        channels: 8,
        channelFormat: 'surround7_1',
      },
    ],
    outputs: [
      {
        id: 'audio',
        name: 'Main',
        kind: 'main',
        channels: 8,
        channelFormat: 'surround7_1',
      },
    ],
  };
  const mainInput = authoredPorts.inputs[0];
  if (!mainInput) throw new Error('fixture input bus is missing');
  const changedPorts: ProviderPorts = {
    ...authoredPorts,
    inputs: [{ ...mainInput, channels: 6, channelFormat: 'surround5_1' }],
  };
  const processor: ExternalProcessorDescriptor = {
    displayName: 'Effect',
    format: 'vst3',
    hasEditor: false,
    layouts: [changedPorts, stereoPorts],
    defaultLayoutId: stereoPorts.layoutId,
    parameters: [],
    typeId: 'vst3:effect',
    vendor: 'Test',
    version: '1',
  };
  const saved: RuntimeGraphDocument = {
    edges: [],
    nodes: [
      {
        config: { ports: authoredPorts, typeId: processor.typeId },
        id: 'effect',
        label: 'Effect',
        type: 'provider',
      },
    ],
    revision: 3,
    schemaVersion: 2,
  };
  const restored = runtimeGraphToAudioGraph(saved, undefined, [processor]);
  const node = restored.nodes[0];
  expect(node?.appearance.offline).toBe(true);
  expect(node?.appearance.detail).toBe('Processor layout unavailable');
  expect(node?.inputPorts).toHaveLength(8);
  expect(node?.outputPorts).toHaveLength(8);
  expect(node?.runtime?.config).toEqual(saved.nodes[0]?.config);
  expect(audioGraphToRuntimeGraph(restored, saved).nodes[0]?.config).toEqual(
    saved.nodes[0]?.config,
  );

  const renamedPorts: ProviderPorts = {
    ...authoredPorts,
    inputs: authoredPorts.inputs.map((bus) => ({ ...bus, name: `Localized ${bus.name}` })),
    outputs: authoredPorts.outputs.map((bus) => ({ ...bus, name: `Localized ${bus.name}` })),
  };
  const renamedDescriptor = { ...processor, layouts: [renamedPorts, stereoPorts] };
  const renamed = runtimeGraphToAudioGraph(saved, undefined, [renamedDescriptor]);
  expect(renamed.nodes[0]?.appearance.offline).toBeUndefined();
  expect(renamed.nodes[0]?.inputPorts[0]?.label).toBe('Main · Main 1');
  expect(renamed.nodes[0]?.runtime?.config).toEqual(saved.nodes[0]?.config);
});

it('treats null provider ports as absent and selects the catalog default without changing parameters', () => {
  const processor: ExternalProcessorDescriptor = {
    displayName: 'Effect',
    format: 'vst3',
    hasEditor: false,
    layouts: [stereoPorts],
    defaultLayoutId: stereoPorts.layoutId,
    parameters: [],
    typeId: 'vst3:null-ports',
    vendor: 'Test',
    version: '1',
  };
  const saved: RuntimeGraphDocument = {
    edges: [],
    nodes: [
      {
        config: {
          parameters: { stateRef: { sha256: 'a'.repeat(64), byteLength: 8 } },
          ports: null,
          typeId: processor.typeId,
        },
        id: 'effect',
        label: 'Effect',
        type: 'provider',
      },
    ],
    revision: 1,
    schemaVersion: 2,
  };
  const model = runtimeGraphToAudioGraph(saved, undefined, [processor]);
  expect(model.nodes[0]?.appearance.offline).toBeUndefined();
  expect(model.nodes[0]?.inputPorts).toHaveLength(2);
  expect(model.nodes[0]?.runtime?.config).toMatchObject({
    typeId: processor.typeId,
    ports: stereoPorts,
    parameters: { stateRef: { sha256: 'a'.repeat(64), byteLength: 8 } },
  });
});

it('rejects provider bus geometry outside the native 64-channel direction bound', () => {
  expect(
    providerPortsFrom({
      layoutId: 'layout-v1:too-wide',
      inputs: [
        { id: 'audio', name: 'Main', kind: 'main', channels: 64, channelFormat: 'discrete' },
        { id: 'bus:1', name: 'Aux', kind: 'auxiliary', channels: 1, channelFormat: 'mono' },
      ],
      outputs: [{ id: 'audio', name: 'Main', kind: 'main', channels: 2, channelFormat: 'stereo' }],
    }),
  ).toBeNull();
  expect(
    providerPortsFrom({
      layoutId: 'layout-v1:bad-stereo',
      inputs: [{ id: 'audio', name: 'Main', kind: 'main', channels: 1, channelFormat: 'stereo' }],
      outputs: [{ id: 'output', name: 'Main', kind: 'main', channels: 2, channelFormat: 'stereo' }],
    }),
  ).toBeNull();
  expect(
    providerPortsFrom({
      layoutId: 'layout-v1:empty-name',
      inputs: [
        { id: 'custom-input', name: '', kind: 'main', channels: 2, channelFormat: 'stereo' },
      ],
      outputs: [
        { id: 'custom-output', name: '', kind: 'main', channels: 2, channelFormat: 'stereo' },
      ],
    }),
  ).toEqual({
    layoutId: 'layout-v1:empty-name',
    inputs: [{ id: 'custom-input', name: '', kind: 'main', channels: 2, channelFormat: 'stereo' }],
    outputs: [
      { id: 'custom-output', name: '', kind: 'main', channels: 2, channelFormat: 'stereo' },
    ],
  });
});

it('names runtime ports from the translated presentation instead of fixed English text', () => {
  const english = runtimeGraphToAudioGraph(gainFixture());
  const gain = english.nodes.find((node) => node.id === 'output-gain');
  expect(gain?.inputPorts.map((port) => port.label)).toEqual(['Gain input 1', 'Gain input 2']);

  const korean = runtimeGraphToAudioGraph(gainFixture(), {
    ...defaultPresentation,
    ports: {
      application: '앱 오디오',
      deviceInput: '장치 입력',
      deviceOutput: '장치 출력',
      effectInput: '{{name}} 입력',
      effectOutput: '{{name}} 출력',
      gainInput: '게인 입력',
      gainOutput: '게인 출력',
      mixerInput: '믹서 입력',
      mixerOutput: '믹서 출력',
    },
  });
  const translated = korean.nodes.find((node) => node.id === 'output-gain');
  expect(translated?.inputPorts.map((port) => port.label)).toEqual(['게인 입력 1', '게인 입력 2']);
  expect(translated?.outputPorts[0]?.label).toBe('게인 출력 1');
  expect(translated?.inputPorts.map((port) => port.id)).toEqual(
    gain?.inputPorts.map((port) => port.id),
  );
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
