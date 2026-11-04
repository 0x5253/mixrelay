import { expect, it } from 'vitest';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { addRoutingNode } from './routingNodeCreation';
import { updateRecorderChannels } from './routingNodeGeometry';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';

it('adds a stereo recorder output and persists channels, never per-take settings', () => {
  const document: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
  const next = addRoutingNode(
    runtimeGraphToAudioGraph(document),
    { kind: 'recorder' },
    { x: 0, y: 0 },
  );
  expect(next.nodes[0]?.inputPorts).toHaveLength(2);
  expect(next.nodes[0]?.outputPorts).toHaveLength(0);
  const saved = audioGraphToRuntimeGraph(next, document);
  expect(saved.nodes[0]).toMatchObject({ type: 'recorder', config: { channels: 2 } });
  expect(Object.keys(saved.nodes[0]?.config ?? {})).toEqual(['channels']);
  expect(runtimeGraphToAudioGraph(saved).nodes[0]?.inputPorts).toHaveLength(2);
});

it('projects the stored recorder channel width without replacing its label', () => {
  const document: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: [{ id: 'take', type: 'recorder', config: { channels: 8 }, label: 'Archive' }],
  };
  const view = runtimeGraphToAudioGraph(document);
  expect(view.nodes[0]?.inputPorts).toHaveLength(8);
  expect(view.nodes[0]?.appearance.title).toBe('Archive');
  expect(audioGraphToRuntimeGraph(view, document).nodes).toEqual(document.nodes);
});

function routedRecorder(hiddenProvider = false): RuntimeGraphDocument {
  return {
    schemaVersion: 2,
    revision: 4,
    nodes: [
      {
        id: 'source',
        label: 'Input',
        type: hiddenProvider ? 'provider' : 'audio_device_input',
        config: hiddenProvider
          ? { typeId: 'missing', parameters: {} }
          : { outputChannels: 8, selector: { match: 'persistent_uid', uid: 'input' } },
      },
      { id: 'take', label: 'Archive', type: 'recorder', config: { channels: 8 } },
      { id: 'other', label: 'Other', type: 'recorder', config: { channels: 8 } },
    ],
    edges: [
      {
        id: 'record-route',
        source: { nodeId: 'source', portId: 'audio' },
        destination: { nodeId: 'take', portId: 'audio' },
        channelMapping: {
          mode: 'explicit',
          routes: [
            { sourceChannel: 0, destinationChannel: 0 },
            { sourceChannel: 7, destinationChannel: 7 },
          ],
        },
      },
      {
        id: 'other-route',
        source: { nodeId: 'source', portId: 'audio' },
        destination: { nodeId: 'other', portId: 'audio' },
        channelMapping: { mode: 'auto' },
      },
    ],
  };
}

it.each([false, true])(
  'shrinks recorder geometry and only removes out-of-range routes (missing provider %s)',
  (hidden) => {
    const document = routedRecorder(hidden);
    const model = runtimeGraphToAudioGraph(document);
    const changed = updateRecorderChannels(model, 'take', 1);
    expect(changed.revision).toBe(model.revision + 1);
    const node = changed.nodes.find((item) => item.id === 'take');
    expect(node?.inputPorts).toHaveLength(1);
    expect(node?.appearance.meterBanks).toEqual([{ channels: 1, nodeId: 'take', portId: 'audio' }]);
    const saved = audioGraphToRuntimeGraph(changed, document);
    expect(saved.nodes.find((item) => item.id === 'take')?.config).toEqual({ channels: 1 });
    const routes = saved.edges.filter((edge) => edge.destination.nodeId === 'take');
    expect(routes).toHaveLength(1);
    expect(routes[0]?.channelMapping).toEqual({
      mode: 'explicit',
      routes: [{ sourceChannel: 0, destinationChannel: 0 }],
    });
    expect(saved.edges.find((edge) => edge.id === 'other-route')).toEqual(document.edges[1]);
  },
);

it.each([1, 2, 3, 4, 5, 6, 7, 8])(
  'supports %s recorder channels without resetting its position',
  (channels) => {
    const model = runtimeGraphToAudioGraph(routedRecorder());
    const changed = updateRecorderChannels(model, 'take', channels);
    expect(changed.nodes.find((node) => node.id === 'take')?.inputPorts).toHaveLength(channels);
    expect(changed.nodes.find((node) => node.id === 'take')?.position).toEqual(
      model.nodes.find((node) => node.id === 'take')?.position,
    );
  },
);

it.each([0, 9, 1.5, Number.NaN])(
  'rejects invalid channel count %s without editing the graph',
  (channels) => {
    const model = runtimeGraphToAudioGraph(routedRecorder());
    expect(updateRecorderChannels(model, 'take', channels)).toBe(model);
  },
);
