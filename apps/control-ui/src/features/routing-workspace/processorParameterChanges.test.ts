import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { changedProcessorParameters } from './processorParameterChanges';
import { addRoutingNode } from './routingNodeCreation';
import { updateRoutingNodeParameter } from './routingNodeParameters';
import { runtimeGraphToAudioGraph } from './runtimeGraphProjection';

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

it('does not publish synthetic or malformed external parameter IDs', () => {
  const base = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'provider', processor, ports },
    position,
  );
  const first = node(base);
  const control = {
    label: 'Parameter',
    minimum: 0,
    maximum: 1,
    step: 0.01,
    suffix: '',
    value: 0.5,
  };
  const before = {
    ...base,
    nodes: [
      {
        ...first,
        appearance: { ...first.appearance, parameterControls: [{ ...control, id: 'external:1' }] },
      },
    ],
  };
  const after = {
    ...before,
    nodes: [
      {
        ...node(before),
        appearance: {
          ...first.appearance,
          parameterControls: ['internal', 'external:-1', 'external:1.2', 'external:nope'].map(
            (id) => ({ ...control, id, value: 0.7 }),
          ),
        },
      },
    ],
  };
  expect(changedProcessorParameters(before, after)).toEqual([]);
});

it('does not invent parameter changes when a restored effect has no authored controls', () => {
  const model = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'provider', processor, ports },
    position,
  );
  expect(changedProcessorParameters(model, model)).toEqual([]);
  expect(updateRoutingNodeParameter(model, node(model).id, 'external:0', 0.5)).toBe(model);
});

it('detects parameter changes when a processor descriptor newly exposes or removes controls', () => {
  const created = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'provider', processor, ports },
    position,
  );
  const first = node(created);
  const after = {
    ...created,
    nodes: [
      {
        ...first,
        appearance: {
          ...first.appearance,
          parameterControls: [
            {
              id: 'external:9',
              label: 'Wet',
              minimum: 0,
              maximum: 1,
              step: 0.01,
              suffix: '',
              value: 0.5,
            },
          ],
        },
      },
    ],
  };
  const before = {
    ...created,
    nodes: [{ ...first, appearance: { ...first.appearance, parameterControls: undefined } }],
  };
  expect(changedProcessorParameters(before, after)).toEqual([
    { nodeId: first.id, typeId: 'effect', parameterId: 9, value: 0.5 },
  ]);
  expect(changedProcessorParameters(after, before)).toEqual([]);
});
