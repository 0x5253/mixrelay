import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument, RuntimeGraphNode } from '../../core/domain/control/graph';
import { addRoutingNode } from './routingNodeCreation';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import { duplexReference, virtualSpec } from './virtualEndpointFixtures';

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

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
const position = { x: 40, y: 60 };
function documentWith(...nodes: RuntimeGraphNode[]): RuntimeGraphDocument {
  return { ...empty, nodes };
}
function node(model: AudioGraphModel): AudioGraphNode {
  const first = model.nodes[0];
  if (!first) throw new Error('Expected a materialized fixture node');
  return first;
}

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

it('keeps synthetic virtual side IDs distinct from authored node IDs', () => {
  const endpointId = 'virtual';
  const escaped = '@virtual:["virtual","source"]';
  const original = documentWith(
    {
      id: endpointId,
      type: 'virtual_endpoint',
      label: null,
      config: { ...virtualSpec, reference: { render: true, capture: false } },
    },
    { id: `${endpointId}::host-source`, type: 'gain', label: null, config: {} },
    { id: escaped, type: 'gain', label: null, config: {} },
  );
  const model = runtimeGraphToAudioGraph(original);
  expect(node(model).id).toBe(`#${escaped}`);
  expect(new Set(model.nodes.map((item) => item.id)).size).toBe(3);
  expect(audioGraphToRuntimeGraph(model, original).nodes.map((item) => item.id)).toEqual(
    original.nodes.map((item) => item.id),
  );
});

it.each(['audio_device_input', 'audio_device_output', 'application_capture'])(
  'renders incomplete %s without fabricating a selector in its runtime document',
  (type) => {
    const original = documentWith({ id: 'incomplete', type, label: null, config: null });
    const model = runtimeGraphToAudioGraph(original);
    expect(node(model).appearance.title).toBe('incomplete');
    expect(node(model).inputPorts).toEqual([]);
    expect(node(model).outputPorts).toEqual([]);
    expect(audioGraphToRuntimeGraph(model, original).nodes[0]?.config).toBeNull();
  },
);

it('omits a view without runtime metadata from the audio document', () => {
  const original = documentWith({ id: 'gain', type: 'gain', label: 'Gain', config: { gainDb: 0 } });
  const model = runtimeGraphToAudioGraph(original);
  const viewOnly = { ...model, nodes: [{ ...node(model), runtime: undefined }] };
  expect(audioGraphToRuntimeGraph(viewOnly, original).nodes).toEqual([]);
});

it('serializes a new virtual reference without a source document and repairs missing metadata', () => {
  const created = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'virtual_input', spec: virtualSpec },
    position,
  );
  expect(audioGraphToRuntimeGraph(created, empty).nodes[0]?.label).toBe(virtualSpec.displayName);
  const first = node(created);
  if (!first.runtime) throw new Error('fixture runtime');
  const malformed = {
    ...created,
    nodes: [{ ...first, runtime: { ...first.runtime, config: null } }],
  };
  expect(audioGraphToRuntimeGraph(malformed, empty).nodes[0]?.config).toMatchObject({
    reference: { render: true, capture: false },
  });
});

it('drops cables whose port metadata or destination is unavailable', () => {
  const original = documentWith({ id: 'gain', type: 'gain', label: null, config: { channels: 2 } });
  const model = runtimeGraphToAudioGraph(original);
  const first = node(model);
  const bare = {
    ...model,
    nodes: [{ ...first, outputPorts: [{ id: 'bare-out', label: 'Output' }] }],
    edges: [
      {
        id: 'bare',
        sourceNodeId: first.id,
        sourcePortId: 'bare-out',
        targetNodeId: 'absent',
        targetPortId: 'bare-in',
      },
    ],
  };
  expect(audioGraphToRuntimeGraph(bare, original).edges).toEqual([]);
});
