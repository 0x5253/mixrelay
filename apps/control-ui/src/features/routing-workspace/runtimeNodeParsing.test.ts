import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument, RuntimeGraphNode } from '../../core/domain/control/graph';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { defaultPresentation } from './graphPresentation';
import { updateRoutingNodeParameter } from './routingNodeParameters';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';
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

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
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

it('uses authored mixer channels and ignores non-string input IDs from malformed state', () => {
  const model = runtimeGraphToAudioGraph(
    documentWith(
      {
        id: 'mixer-a',
        type: 'mixer',
        label: null,
        config: { channels: 2, inputs: ['voice', 2, 'music'] },
      },
      { id: 'mixer-b', type: 'mixer', label: 'Empty', config: null },
    ),
  );
  expect(node(model).appearance.title).toBe('Mixer');
  expect(node(model).inputPorts.map((port) => port.runtimePortId)).toEqual([
    'voice',
    'voice',
    'music',
    'music',
  ]);
  expect(model.nodes[1]?.inputPorts).toEqual([]);
  expect(audioGraphToRuntimeGraph(model, empty).nodes[1]?.config).toBeNull();
});

it.each(['compressor', 'limiter'] as const)(
  'keeps incomplete %s unsupported without inventing channel or parameter defaults',
  (type) => {
    const original = documentWith({ id: type, type, label: null, config: { bypass: false } });
    const model = runtimeGraphToAudioGraph(original);
    expect(node(model).appearance.category).toBe('Unsupported');
    expect(node(model).appearance.parameterControls).toBeUndefined();
    expect(node(model).inputPorts).toEqual([]);
    expect(node(model).outputPorts).toEqual([]);
    expect(audioGraphToRuntimeGraph(model, original).nodes[0]?.config).toEqual({ bypass: false });
  },
);

it.each(
  (['compressor', 'limiter'] as const).flatMap((type) =>
    [null, {}, { bypass: 'false' }, { bypass: 0 }, { bypass: null }].map(
      (config) => [type, config] as const,
    ),
  ),
)('keeps %s state without a boolean bypass unsupported and unedited: %j', (type, config) => {
  const original = documentWith({ id: type, type, label: 'Saved', config });
  const model = runtimeGraphToAudioGraph(original);
  expect(node(model).appearance).toMatchObject({ category: 'Unsupported', title: 'Saved' });
  expect(node(model).appearance.parameterControls).toBeUndefined();
  const parameter = type === 'limiter' ? 'ceilingDb' : 'thresholdDb';
  expect(updateRoutingNodeParameter(model, type, parameter, -6)).toBe(model);
  expect(audioGraphToRuntimeGraph(model, original).nodes[0]?.config).toEqual(config);
});

it('marks an installed effect with no usable layout unavailable instead of inventing ports', () => {
  const original = documentWith({
    id: 'effect-node',
    type: 'provider',
    label: null,
    config: { typeId: processor.typeId, parameters: { parameterValues: { '1': 'invalid' } } },
  });
  const model = runtimeGraphToAudioGraph(original, undefined, [{ ...processor, layouts: [] }]);
  expect(node(model).appearance).toMatchObject({ title: 'Effect', offline: true });
  expect(node(model).inputPorts).toEqual([]);
  expect(node(model).outputPorts).toEqual([]);
  expect(audioGraphToRuntimeGraph(model, original).nodes[0]?.config).toEqual(
    original.nodes[0]?.config,
  );
});

it('keeps malformed authored effect layout unavailable rather than replacing it with a catalog default', () => {
  const original = documentWith({
    id: 'effect-node',
    type: 'provider',
    label: 'Stored effect',
    config: { typeId: processor.typeId, ports: { ...ports, outputs: [] } },
  });
  const model = runtimeGraphToAudioGraph(original, undefined, [processor]);
  expect(node(model).appearance.offline).toBe(true);
  expect(node(model).outputPorts).toEqual([]);
  expect(audioGraphToRuntimeGraph(model, original).nodes[0]?.config).toEqual(
    original.nodes[0]?.config,
  );
});

it('retains unknown effect identity and authored geometry when its catalog entry disappears', () => {
  const original = documentWith({ id: 'lost-effect', type: 'provider', label: null, config: null });
  const model = runtimeGraphToAudioGraph(original);
  expect(node(model).appearance).toMatchObject({ title: 'lost-effect', offline: true });
  expect(audioGraphToRuntimeGraph(model, original).nodes).toEqual(original.nodes);
});

it('ignores a virtual endpoint without an authored render/capture reference', () => {
  const original = documentWith({
    id: 'virtual',
    type: 'virtual_endpoint',
    label: null,
    config: virtualSpec,
  });
  expect(runtimeGraphToAudioGraph(original).nodes).toEqual([]);
});

it('presents unnamed current source selectors without adding defaults to their runtime config', () => {
  const original = documentWith(
    {
      id: 'app',
      type: 'application_capture',
      label: null,
      config: {
        selector: { match: 'bundle_id', bundle_id: 'example.app', include_descendants: true },
        stream: { channels: 2, channelLayout: 'stereo' },
        sourcePlayback: 'preserve',
      },
    },
    {
      id: 'input',
      type: 'audio_device_input',
      label: null,
      config: { selector: { match: 'default_input' }, outputChannels: 2 },
    },
    {
      id: 'output',
      type: 'audio_device_output',
      label: null,
      config: { selector: { match: 'default_output' }, inputChannels: 2 },
    },
  );
  const model = runtimeGraphToAudioGraph(original);
  expect(model.nodes.map((view) => view.appearance.title)).toEqual(['app', 'input', 'output']);
  expect(model.nodes.map((view) => view.appearance.gainControl?.valueDb)).toEqual([
    0,
    0,
    undefined,
  ]);
  expect(model.nodes.map((view) => [view.inputPorts.length, view.outputPorts.length])).toEqual([
    [0, 2],
    [0, 2],
    [2, 0],
  ]);
  expect(audioGraphToRuntimeGraph(model, original).nodes.map((saved) => saved.config)).toEqual(
    original.nodes.map((saved) => saved.config),
  );
});

it('uses the default recorder title for a saved unnamed recorder', () => {
  const model = runtimeGraphToAudioGraph(
    documentWith({ id: 'rec', type: 'recorder', label: null, config: { channels: 2 } }),
  );
  expect(node(model).appearance.title).toBe('Recorder');
  expect(node(model).inputPorts).toHaveLength(2);
});

it.each([null, {}, { channels: 0 }, { channels: 1.5 }, { channels: 65 }])(
  'does not invent recorder channels for invalid config %j',
  (config) => {
    const original = documentWith({ id: 'rec', type: 'recorder', label: null, config });
    const model = runtimeGraphToAudioGraph(original);
    expect(node(model).inputPorts).toEqual([]);
    expect(audioGraphToRuntimeGraph(model, original).nodes).toEqual(original.nodes);
  },
);

it('omits a virtual endpoint whose stored config has no display name', () => {
  const config = {
    ...virtualSpec,
    displayName: undefined,
    reference: { render: true, capture: true },
  };
  const model = runtimeGraphToAudioGraph(
    documentWith({
      id: 'virtual',
      type: 'virtual_endpoint',
      label: virtualSpec.displayName,
      config,
    }),
  );
  expect(model.nodes).toEqual([]);
});
