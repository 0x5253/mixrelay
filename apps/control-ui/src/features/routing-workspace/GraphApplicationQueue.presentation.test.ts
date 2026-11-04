import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import { moveAudioGraphNode } from '../../core/audio-graph/model';
import type { GraphLayoutResponse } from '../../core/domain/control/catalog';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { graphWriterDefaults } from '../../test-support/portDefaults';
import { workspaceRuntime } from '../../test-support/workspaceRuntime';
import { addRoutingNode } from './routingNodeCreation';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 8, nodes: [], edges: [] };
const flush = () => vi.advanceTimersByTimeAsync(0);
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function fixture() {
  let graphId = 'graph-1';
  let model = addRoutingNode(
    runtimeGraphToAudioGraph({ ...empty, revision: 1 }),
    { kind: 'effect', effect: 'equalizer' },
    { x: 120, y: 80 },
  );
  const savedLayout = Promise.withResolvers<GraphLayoutResponse>();
  const getGraph = vi.fn().mockResolvedValue(empty);
  const port = {
    ...graphWriterDefaults(getGraph, () => graphId),
    getGraph,
    getGraphStatus: vi.fn().mockResolvedValue({ activeRevision: 8, submittedRevision: 8 }),
    applyGraph: vi.fn(),
  };
  const callbacks = {
    getGraphId: () => graphId,
    getDocument: () => empty,
    getModel: () => model,
    document: vi.fn(),
    getReloadLayout: vi.fn(() => savedLayout.promise),
    restore: vi.fn(),
    state: vi.fn(),
    error: vi.fn(),
  };
  const queue = new GraphApplicationQueue(port, callbacks, workspaceRuntime);
  const result = {
    queue,
    port,
    callbacks,
    savedLayout,
    get model() {
      return model;
    },
    refreshPresentation() {
      // useWorkspaceGraph performs this exact projection after its document changes.
      const previous = model;
      const refreshed = runtimeGraphToAudioGraph(audioGraphToRuntimeGraph(previous, empty));
      model = {
        ...refreshed,
        nodes: refreshed.nodes.map((node) => {
          const old = previous.nodes.find((candidate) => candidate.id === node.id);
          return { ...node, position: old?.position ?? node.position, size: old?.size };
        }),
      };
    },
    edit() {
      model = addRoutingNode(model, { kind: 'effect', effect: 'delay' }, { x: 480, y: 80 });
      queue.submit(model);
    },
    move() {
      model = moveAudioGraphNode(model, model.nodes[0]?.id ?? '', { x: 240, y: 160 });
    },
    selectGraph(id: string) {
      graphId = id;
    },
  };
  callbacks.document.mockImplementation(() => result.refreshPresentation());
  return result;
}

it('accepts reload when document publication only refreshed presentation', async () => {
  const f = fixture();
  const initial = f.model;
  const authored = audioGraphToRuntimeGraph(initial, empty);
  f.queue.reload();
  await flush();
  expect(f.callbacks.getReloadLayout).toHaveBeenCalledExactlyOnceWith('graph-1');
  expect(f.callbacks.document).not.toHaveBeenCalled();
  expect(f.model).toBe(initial);
  f.savedLayout.resolve({ graphId: 'graph-1', layoutRevision: 1, layout: { positions: {} } });
  await flush();
  expect(f.callbacks.document).toHaveBeenCalledWith(empty);
  expect(f.model).not.toBe(initial);
  expect(audioGraphToRuntimeGraph(f.model, empty)).toEqual(authored);
  expect(f.model.nodes.map((node) => node.position)).toEqual(
    initial.nodes.map((node) => node.position),
  );
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(empty, {
    positions: {},
  });
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('preserves an authored edit made after presentation refresh while reload is pending', async () => {
  const f = fixture();
  f.queue.reload();
  await flush();
  f.refreshPresentation();
  f.edit();
  const edited = audioGraphToRuntimeGraph(f.model, empty);
  f.savedLayout.resolve({ graphId: 'graph-1', layoutRevision: 1, layout: { positions: {} } });
  await flush();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_draft_changed');
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  expect(audioGraphToRuntimeGraph(f.model, empty)).toEqual(edited);
  expect(vi.getTimerCount()).toBe(0);
});

it('preserves layout edits without an audio submission while reload is pending', async () => {
  const f = fixture();
  f.queue.reload();
  await flush();
  f.move();
  const position = f.model.nodes[0]?.position;
  f.savedLayout.resolve({ graphId: 'graph-1', layoutRevision: 1, layout: { positions: {} } });
  await flush();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_draft_changed');
  expect(f.model.nodes[0]?.position).toEqual(position);
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it('does not restore when document publication retires the reload writer', async () => {
  const f = fixture();
  f.callbacks.document.mockImplementation(() => f.queue.reset(empty));
  f.queue.reload();
  await flush();
  f.savedLayout.resolve({ graphId: 'graph-1', layoutRevision: 1, layout: { positions: {} } });
  await flush();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).not.toHaveBeenCalledWith('active');
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it('does not restore into a graph selected by document publication', async () => {
  const f = fixture();
  f.callbacks.document.mockImplementation(() => f.selectGraph('graph-2'));
  f.queue.reload();
  await flush();
  f.savedLayout.resolve({ graphId: 'graph-1', layoutRevision: 1, layout: { positions: {} } });
  await flush();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  // This retired owner must not publish its error into the newly selected graph.
  expect(f.callbacks.error).toHaveBeenCalledExactlyOnceWith(null);
  expect(f.port.getGraph).toHaveBeenCalledTimes(1);
  expect(f.callbacks.state).not.toHaveBeenCalledWith('active');
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});
