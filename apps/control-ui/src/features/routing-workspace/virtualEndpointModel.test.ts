import { expect, it } from 'vitest';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import { serializeGraphEdges } from './graphEdgeProjection';
import { addRoutingNode } from './routingNodeCreation';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import { duplexReference, virtualSpec } from './virtualEndpointFixtures';
import { virtualEndpointNodeRequest } from './virtualEndpointMenu';

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
const position = { x: 0, y: 0 };

it.each(['render', 'capture'] as const)(
  'materializes a %s-only device without creating its absent stream',
  (side) => {
    const spec = {
      ...virtualSpec,
      render: side === 'render' ? virtualSpec.render : null,
      capture: side === 'capture' ? virtualSpec.capture : null,
    };
    const request = virtualEndpointNodeRequest(side === 'capture' ? 'input' : 'output', spec);
    if (!request) throw new Error('Expected legal directional reference');
    expect(virtualEndpointNodeRequest(side === 'capture' ? 'output' : 'input', spec)).toBeNull();
    const model = addRoutingNode(runtimeGraphToAudioGraph(empty), request, position);
    const node = model.nodes[0];
    if (!node) throw new Error('Expected directional view');
    const ports = side === 'capture' ? node.outputPorts : node.inputPorts;
    expect(ports).toHaveLength(spec[side]?.channels ?? 0);
    expect(
      ports.every((port) => port.runtimePortId === (side === 'capture' ? 'from_os' : 'to_os')),
    ).toBe(true);
    expect(audioGraphToRuntimeGraph(model, empty).nodes[0]?.config).toEqual(
      expect.objectContaining({ render: spec.render, capture: spec.capture }),
    );
  },
);

it.each(['source', 'sink'] as const)(
  'removes only the deleted %s from unresolved routes',
  (removedSide) => {
    const graph: RuntimeGraphDocument = {
      ...empty,
      nodes: [
        {
          id: 'device',
          label: null,
          type: 'virtual_endpoint',
          config: { ...virtualSpec, reference: duplexReference },
        },
        { id: 'fx', label: null, type: 'provider', config: { typeId: 'missing' } },
      ],
      edges: [
        {
          id: 'into-device',
          source: { nodeId: 'fx', portId: 'audio' },
          destination: { nodeId: 'device', portId: 'to_os' },
          channelMapping: {
            mode: 'explicit',
            routes: [
              { sourceChannel: 0, destinationChannel: 0 },
              { sourceChannel: 1, destinationChannel: 13 },
            ],
          },
        },
        {
          id: 'out-device',
          source: { nodeId: 'device', portId: 'from_os' },
          destination: { nodeId: 'fx', portId: 'audio' },
          channelMapping: { mode: 'auto' },
        },
      ],
    };
    const model = runtimeGraphToAudioGraph(graph);
    const removedSink = {
      ...model,
      nodes: model.nodes.filter((node) => node.runtime?.binding?.side !== removedSide),
    };
    expect(audioGraphToRuntimeGraph(removedSink, graph).edges).toEqual([
      graph.edges[removedSide === 'sink' ? 1 : 0],
    ]);
  },
);

it('filters unresolved routes individually to the channels a stored device still has', () => {
  const graph: RuntimeGraphDocument = {
    ...empty,
    nodes: [
      {
        id: 'device',
        label: null,
        type: 'virtual_endpoint',
        config: {
          ...virtualSpec,
          render: { channels: 2, channelLayout: 'stereo' },
          reference: duplexReference,
        },
      },
      { id: 'fx', label: null, type: 'provider', config: { typeId: 'missing' } },
    ],
    edges: [
      {
        id: 'into-device',
        source: { nodeId: 'fx', portId: 'audio' },
        destination: { nodeId: 'device', portId: 'to_os' },
        channelMapping: {
          mode: 'explicit',
          routes: [
            { sourceChannel: 0, destinationChannel: 0 },
            { sourceChannel: 1, destinationChannel: 13 },
          ],
        },
      },
      {
        id: 'out-device',
        source: { nodeId: 'device', portId: 'from_os' },
        destination: { nodeId: 'fx', portId: 'audio' },
        channelMapping: {
          mode: 'explicit',
          routes: [
            { sourceChannel: 15, destinationChannel: 0 },
            { sourceChannel: 16, destinationChannel: 1 },
          ],
        },
      },
    ],
  };
  expect(audioGraphToRuntimeGraph(runtimeGraphToAudioGraph(graph), graph).edges).toEqual([
    {
      ...graph.edges[0],
      channelMapping: { mode: 'explicit', routes: [{ sourceChannel: 0, destinationChannel: 0 }] },
    },
    {
      ...graph.edges[1],
      channelMapping: { mode: 'explicit', routes: [{ sourceChannel: 15, destinationChannel: 0 }] },
    },
  ]);
});

it('retains edited provider state and nullable labels when its descriptor becomes unavailable', () => {
  const original: RuntimeGraphDocument = {
    ...empty,
    nodes: [
      {
        id: 'fx',
        label: null,
        type: 'provider',
        config: { typeId: 'fixture', parameters: { parameterValues: { '42': 0.2 } } },
      },
    ],
  };
  const draft: RuntimeGraphDocument = {
    ...original,
    nodes: [
      {
        ...original.nodes[0],
        id: 'fx',
        label: null,
        type: 'provider',
        config: { typeId: 'fixture', parameters: { parameterValues: { '42': 0.8 } } },
      },
    ],
  };
  const placeholder = runtimeGraphToAudioGraph(draft);
  expect(placeholder.nodes[0]?.outputPorts).toEqual([]);
  expect(audioGraphToRuntimeGraph(placeholder, original).nodes).toEqual(draft.nodes);
});

it('does not collide edited cable IDs with preserved authoritative route IDs', () => {
  const original = {
    id: 'existing',
    source: { nodeId: 'a', portId: 'audio' },
    destination: { nodeId: 'b', portId: 'audio' },
    channelMapping: { mode: 'auto' as const },
  };
  const cable = {
    id: 'existing',
    sourceNodeId: 'a',
    sourcePortId: 'audio:0',
    targetNodeId: 'b',
    targetPortId: 'audio:0',
  };
  const result = serializeGraphEdges(
    {
      nodes: [],
      revision: 1,
      edges: [cable],
      edgeProjections: [
        { edge: original, views: [] },
        { edge: { ...original, id: '@route:["existing"]' }, views: [] },
      ],
    },
    new Set(['a', 'b']),
    () => original,
  );
  expect(result[0]).toEqual(original);
  expect(result).toHaveLength(3);
  expect(new Set(result.map((edge) => edge.id)).size).toBe(3);
});

it('keeps N identities distinct while sharing each identity between its input and output references', () => {
  let model = runtimeGraphToAudioGraph(empty);
  const specs = [virtualSpec, { ...virtualSpec, id: '3588a3be-bd13-48ec-ae63-edc7e00ef873' }];
  for (const spec of specs) {
    model = addRoutingNode(model, { kind: 'virtual_input', spec }, position);
    model = addRoutingNode(model, { kind: 'virtual_output', spec }, position);
  }
  const saved = audioGraphToRuntimeGraph(model, empty);
  expect(model.nodes).toHaveLength(4);
  expect(saved.nodes.map((node) => node.config)).toEqual(
    specs.map((spec) => ({ ...spec, gainDb: 0, reference: duplexReference })),
  );
  expect(new Set(saved.nodes.map((node) => node.id)).size).toBe(2);
  const otherGraph = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'virtual_output', spec: virtualSpec },
    position,
  );
  expect(audioGraphToRuntimeGraph(otherGraph, empty).nodes[0]?.config).toMatchObject({
    id: virtualSpec.id,
  });
});

it('removes references without shrinking the native device and removes the graph node only after its final view', () => {
  let model = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'virtual_input', spec: virtualSpec },
    position,
  );
  model = addRoutingNode(model, { kind: 'virtual_output', spec: virtualSpec }, position);
  const remaining = {
    ...model,
    nodes: model.nodes.filter((node) => node.runtime?.binding?.side === 'sink'),
  };
  expect(audioGraphToRuntimeGraph(remaining, empty).nodes[0]?.config).toMatchObject({
    render: virtualSpec.render,
    capture: virtualSpec.capture,
    reference: { render: false, capture: true },
  });
  expect(audioGraphToRuntimeGraph({ ...remaining, nodes: [] }, empty).nodes).toEqual([]);
});

it.each([
  [64, 16],
  [2, 32],
  [8, 8],
])(
  'presents exact graph capture/source and render/sink widths for OS %i/%i',
  (renderChannels, captureChannels) => {
    const spec = {
      ...virtualSpec,
      render: { channels: renderChannels, channelLayout: 'discrete' as const },
      capture: { channels: captureChannels, channelLayout: 'discrete' as const },
    };
    const input = addRoutingNode(
      runtimeGraphToAudioGraph(empty),
      { kind: 'virtual_input', spec },
      position,
    );
    const model = addRoutingNode(input, { kind: 'virtual_output', spec }, position);
    const source = model.nodes.find((node) => node.runtime?.binding?.side === 'source');
    const sink = model.nodes.find((node) => node.runtime?.binding?.side === 'sink');
    if (!source) throw new Error('Source fixture absent');
    if (!sink) throw new Error('Sink fixture absent');
    expect(source.outputPorts).toHaveLength(captureChannels);
    expect(source.appearance.channelCount).toBe(captureChannels);
    expect(source.outputPorts.every((port) => port.runtimePortId === 'from_os')).toBe(true);
    expect(source.outputPorts.map((port) => [port.channelIndex, port.boundary])).toEqual(
      Array.from({ length: captureChannels }, (_, channel) => [channel, 'source']),
    );
    expect(source.inputPorts).toEqual([]);
    expect(sink.inputPorts).toHaveLength(renderChannels);
    expect(sink.appearance.channelCount).toBe(renderChannels);
    expect(sink.inputPorts.every((port) => port.runtimePortId === 'to_os')).toBe(true);
    expect(sink.inputPorts.map((port) => [port.channelIndex, port.boundary])).toEqual(
      Array.from({ length: renderChannels }, (_, channel) => [channel, 'sink']),
    );
    expect(sink.outputPorts).toEqual([]);
    for (const node of model.nodes)
      expect(node.appearance.endpointControl).toMatchObject({ renderChannels, captureChannels });
    const saved = audioGraphToRuntimeGraph(model, empty);
    expect(saved.nodes[0]?.config).toMatchObject({
      render: spec.render,
      capture: spec.capture,
      reference: duplexReference,
    });
    expect(runtimeGraphToAudioGraph(saved).nodes[0]?.appearance.meterBanks).toEqual([
      { channels: captureChannels, nodeId: saved.nodes[0]?.id, portId: 'from_os' },
    ]);
    expect(runtimeGraphToAudioGraph(saved).nodes[1]?.appearance.meterBanks).toEqual([
      { channels: renderChannels, nodeId: saved.nodes[0]?.id, portId: 'to_os' },
    ]);
    expect(audioGraphToRuntimeGraph(runtimeGraphToAudioGraph(saved), saved)).toEqual(saved);
    for (const removed of ['source', 'sink'] as const) {
      const remaining = {
        ...model,
        nodes: model.nodes.filter((node) => node.runtime?.binding?.side !== removed),
      };
      expect(audioGraphToRuntimeGraph(remaining, saved).nodes[0]?.config).toMatchObject({
        render: spec.render,
        capture: spec.capture,
        reference: { render: removed !== 'source', capture: removed !== 'sink' },
      });
    }
  },
);

it('never decodes ordinary node IDs as virtual suffixes, including colliding view names', () => {
  const graph: RuntimeGraphDocument = {
    ...empty,
    nodes: [
      {
        id: 'fx',
        label: null,
        type: 'virtual_endpoint',
        config: { ...virtualSpec, reference: duplexReference },
      },
      {
        id: 'fx::host-source',
        label: 'User processor',
        type: 'gain',
        config: { gainDb: 0, channels: 2 },
      },
      {
        id: '$fx',
        label: null,
        type: 'virtual_endpoint',
        config: { ...virtualSpec, id: 'second-device', reference: duplexReference },
      },
    ],
    edges: [
      {
        id: 'route',
        source: { nodeId: 'fx::host-source', portId: 'audio' },
        destination: { nodeId: 'fx', portId: 'to_os' },
        channelMapping: { mode: 'explicit', routes: [{ sourceChannel: 1, destinationChannel: 3 }] },
      },
    ],
  };
  const model = runtimeGraphToAudioGraph(graph);
  expect(new Set(model.nodes.map((node) => node.id)).size).toBe(model.nodes.length);
  const saved = audioGraphToRuntimeGraph(model, graph);
  expect(saved.nodes.map((node) => node.id)).toEqual(graph.nodes.map((node) => node.id));
  expect(saved.edges[0]).toMatchObject({
    source: graph.edges[0]?.source,
    destination: graph.edges[0]?.destination,
    channelMapping: graph.edges[0]?.channelMapping,
  });
});

it('retains unresolved provider edges through unrelated edits and restores exact channel routes when the catalog arrives', () => {
  const graph: RuntimeGraphDocument = {
    ...empty,
    nodes: [
      {
        id: 'device',
        label: null,
        type: 'virtual_endpoint',
        config: { ...virtualSpec, reference: duplexReference },
      },
      { id: 'fx', label: 'Effect', type: 'provider', config: { typeId: 'test:effect' } },
    ],
    edges: [
      {
        id: 'kept',
        source: { nodeId: 'fx', portId: 'audio' },
        destination: { nodeId: 'device', portId: 'to_os' },
        channelMapping: {
          mode: 'explicit',
          routes: [{ sourceChannel: 1, destinationChannel: 13 }],
        },
      },
    ],
  };
  const pending = runtimeGraphToAudioGraph(graph);
  expect(pending.edges).toEqual([]);
  expect(pending.nodes.find((node) => node.id === 'fx')?.outputPorts).toEqual([]);
  const edited = addRoutingNode(pending, { kind: 'compressor' }, position);
  const saved = audioGraphToRuntimeGraph(edited, graph);
  expect(saved.edges).toEqual(graph.edges);
  const descriptor: ExternalProcessorDescriptor = {
    typeId: 'test:effect',
    displayName: 'Effect',
    format: 'vst3',
    layouts: [
      {
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
      },
    ],
    defaultLayoutId: 'layout-v1:stereo',
    parameters: [],
    hasEditor: false,
    vendor: 'Test',
    version: '1',
  };
  const restored = runtimeGraphToAudioGraph(saved, undefined, [descriptor]);
  expect(restored.edges).toHaveLength(1);
  expect(audioGraphToRuntimeGraph(restored, saved).edges[0]).toMatchObject({
    source: graph.edges[0]?.source,
    destination: graph.edges[0]?.destination,
    channelMapping: graph.edges[0]?.channelMapping,
  });
  expect(
    audioGraphToRuntimeGraph(
      { ...pending, nodes: pending.nodes.filter((node) => node.id !== 'fx') },
      graph,
    ).edges,
  ).toEqual([]);
});

it('drops explicit routes that no longer fit authoritative recorder inputs', () => {
  const source: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 1,
    nodes: [
      { id: 'gain', type: 'gain', label: null, config: { channels: 2 } },
      { id: 'rec', type: 'recorder', label: null, config: { channels: 1 } },
    ],
    edges: [
      {
        id: 'stale',
        source: { nodeId: 'gain', portId: 'audio' },
        destination: { nodeId: 'rec', portId: 'audio' },
        channelMapping: { mode: 'explicit', routes: [{ sourceChannel: 0, destinationChannel: 3 }] },
      },
    ],
  };
  expect(audioGraphToRuntimeGraph(runtimeGraphToAudioGraph(source), source).edges).toEqual([]);
});

it('drops authoritative routes when a recorder port lacks its runtime channel', () => {
  const original: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 1,
    nodes: [
      { id: 'gain', type: 'gain', label: null, config: { channels: 2 } },
      { id: 'rec', type: 'recorder', label: null, config: { channels: 1 } },
    ],
    edges: [
      {
        id: 'route',
        source: { nodeId: 'gain', portId: 'audio' },
        destination: { nodeId: 'rec', portId: 'audio' },
        channelMapping: { mode: 'auto' },
      },
    ],
  };
  const model = runtimeGraphToAudioGraph(original);
  const incomplete = {
    ...model,
    nodes: model.nodes.map((node) =>
      node.id === 'rec'
        ? {
            ...node,
            inputPorts: node.inputPorts.map((port) => ({ ...port, channelIndex: undefined })),
          }
        : node,
    ),
  };
  expect(audioGraphToRuntimeGraph(incomplete, original).edges).toEqual([]);
  expect(
    audioGraphToRuntimeGraph({ ...incomplete, edgeProjections: undefined }, original).edges,
  ).toEqual([]);
  const unbound = {
    ...model,
    edgeProjections: undefined,
    nodes: model.nodes.map((node) => ({
      ...node,
      inputPorts: node.inputPorts.map((port) => ({ ...port, runtimePortId: undefined })),
    })),
  };
  expect(audioGraphToRuntimeGraph(unbound, original).edges).toEqual([]);
});
