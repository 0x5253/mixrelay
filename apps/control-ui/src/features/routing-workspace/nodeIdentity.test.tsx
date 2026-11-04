import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { removeGraphItems } from '../../core/domain/graph/topology';
import { useUndoHistory } from '../../core/history/useUndoHistory';
import { advance, eventually } from '../../test-support/fakeTimerQueries';
import { workspaceRuntime } from '../../test-support/workspaceRuntime';
import { runtimeGraphToAudioGraph } from './runtimeGraphProjection';

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});
it('node deletion history reapplies the original node IDs through the actual graph application queue', async () => {
  vi.useFakeTimers();
  let document: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: [
      {
        id: 'original-gain',
        type: 'gain',
        label: 'Microphone',
        config: { channels: 2, gainDb: 0 },
      },
    ],
  };
  let model = runtimeGraphToAudioGraph(document);
  const port = new StorybookMockControlPort({
    graph: document,
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      overruns: 0,
      underruns: 0,
    },
  });
  const queue = new GraphApplicationQueue(
    port,
    {
      getGraphId: () => 'graph-1',
      getDocument: () => document,
      getModel: () => model,
      document: (next) => {
        document = next;
      },
      restore: (next) => {
        document = next;
        model = runtimeGraphToAudioGraph(next);
      },
      state: () => {},
      error: () => {},
    },
    workspaceRuntime,
  );
  const history = renderHook(() => useUndoHistory<AudioGraphModel, 'runtime'>());
  const removed = removeGraphItems(model, ['original-gain'], []);
  act(() => history.result.current.record(model, removed, 'runtime'));
  const apply = async (next: AudioGraphModel) => {
    model = next;
    queue.submit(next);
    await advance();
    await eventually(() => expect(queue.canPublishParameters).toBe(true));
  };
  await apply(removed);
  expect((await port.getGraph()).nodes).toEqual([]);
  for (let iteration = 0; iteration < 2; iteration++) {
    const undo = history.result.current.undo();
    if (!undo) throw new Error('undo required');
    await apply(undo.value);
    expect((await port.getGraph()).nodes.map((node) => node.id)).toEqual(['original-gain']);
    const redo = history.result.current.redo();
    if (!redo) throw new Error('redo required');
    await apply(redo.value);
    expect((await port.getGraph()).nodes).toEqual([]);
  }
});

it('a newly added replacement never reuses a deleted identity', async () => {
  const { addRoutingNode } = await import('./routingNodeCreation');
  const empty: AudioGraphModel = { nodes: [], edges: [], revision: 1 };
  const first = addRoutingNode(empty, { kind: 'recorder' }, { x: 0, y: 0 });
  const firstId = first.nodes[0]?.id;
  if (!firstId) throw new Error('created node required');
  const deleted = removeGraphItems(first, [firstId], []);
  const replacement = addRoutingNode(deleted, { kind: 'recorder' }, { x: 0, y: 0 });
  expect(replacement.nodes[0]?.id).not.toBe(firstId);
});
