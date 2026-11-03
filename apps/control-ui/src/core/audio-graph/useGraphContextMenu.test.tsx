import { act, renderHook } from '@testing-library/react';
import { useRef, useState } from 'react';
import { expect, it, vi } from 'vitest';
import { useUndoHistory } from '../history/useUndoHistory';
import type { AudioGraphModel } from './model';
import {
  type AudioFlowEdge,
  type AudioFlowNode,
  reconcileAudioFlowEdges,
  reconcileAudioFlowNodes,
} from './reactFlowAdapter';
import {
  type GraphContextMenuLabels,
  type NodeContextAction,
  useGraphContextMenu,
} from './useGraphContextMenu';

const model: AudioGraphModel = {
  revision: 1,
  nodes: ['a', 'b', 'c'].map((id) => ({
    id,
    position: { x: 0, y: 0 },
    appearance: { title: id, category: '', icon: '' },
    inputPorts: [{ id: 'in', label: 'Input' }],
    outputPorts: [{ id: 'out', label: 'Output' }],
  })),
  edges: [
    { id: 'ab', sourceNodeId: 'a', sourcePortId: 'out', targetNodeId: 'b', targetPortId: 'in' },
    { id: 'cb', sourceNodeId: 'c', sourcePortId: 'out', targetNodeId: 'b', targetPortId: 'in' },
  ],
};
type Port = { nodeId: string; portId: string; side: 'input' | 'output' };
function target(
  kind: 'node' | 'edge' | 'port' | 'selection' | 'canvas',
  id = 'a',
  side: 'input' | 'output' = 'output',
) {
  const element = document.createElement('div');
  if (kind === 'node' || kind === 'edge') {
    element.className = `react-flow__${kind}`;
    element.dataset.id = id;
  }
  if (kind === 'port')
    Object.assign(element.dataset, {
      audioNodeId: id,
      audioPortId: side === 'input' ? 'in' : 'out',
      audioPortSide: side,
    });
  if (kind === 'selection') element.className = 'react-flow__nodesselection-rect';
  return { target: element };
}
function setup(
  selectedNodes: string[] = [],
  selectedEdges: string[] = [],
  ports: Port[] = [],
  renaming: {
    renameNode?: (id: string) => void;
    labels?: GraphContextMenuLabels;
    nodeActions?: (nodeId: string) => readonly NodeContextAction[];
  } = {},
) {
  const committed = vi.fn();
  const fitView = vi.fn();
  const selectPorts = vi.fn();
  const hook = renderHook(() => {
    const [current, setModel] = useState(model);
    const [nodes, setNodes] = useState<AudioFlowNode[]>(() =>
      reconcileAudioFlowNodes([], model).map((n) => ({
        ...n,
        selected: selectedNodes.includes(n.id),
      })),
    );
    const [edges, setEdges] = useState<AudioFlowEdge[]>(() =>
      reconcileAudioFlowEdges([], model).map((e) => ({
        ...e,
        selected: selectedEdges.includes(e.id),
      })),
    );
    const selectedPorts = useRef(ports);
    const history = useUndoHistory<AudioGraphModel, 'runtime'>();
    const menu = useGraphContextMenu({
      ...renaming,
      model: current,
      nodes,
      edges,
      setNodes,
      setEdges,
      selectedPorts,
      fitView,
      addItems: [{ id: 'source', label: 'Source' }],
      selectPorts: (ports) => {
        selectedPorts.current = [...ports];
        selectPorts(ports);
      },
      clearPorts: () => {
        selectedPorts.current = [];
      },
      commit: (next, kind) => {
        history.record(current, next, kind);
        setModel(next);
        committed(next, kind);
      },
    });
    return { menu, nodes, edges, current, selectedPorts, history };
  });
  return { ...hook, committed, fitView, selectPorts };
}

it('deletes a node and all incident cables in one undoable mutation', () => {
  const f = setup();
  act(() => f.result.current.menu.prepare(target('node', 'b')));
  act(() => f.result.current.menu.execute('context:remove'));
  expect(f.committed).toHaveBeenCalledTimes(1);
  expect(f.result.current.current.nodes.map((n) => n.id)).toEqual(['a', 'c']);
  expect(f.result.current.current.edges).toEqual([]);
  expect(f.result.current.current.revision).toBe(2);
  expect(f.result.current.history.undo()?.value).toEqual(model);
  expect(f.result.current.history.undo()).toBeNull();
  expect(f.result.current.history.redo()?.value.nodes.map((n) => n.id)).toEqual(['a', 'c']);
});

it('preserves selected nodes and cables when right-clicking the selection rectangle', () => {
  const f = setup(['a', 'c'], ['cb']);
  act(() => f.result.current.menu.prepare(target('selection')));
  expect(f.result.current.menu.items.map((i) => i.id)).toEqual([
    'context:disconnect',
    'context:remove',
  ]);
  act(() => f.result.current.menu.execute('context:remove'));
  expect(f.result.current.current.nodes.map((n) => n.id)).toEqual(['b']);
  expect(f.result.current.current.edges).toEqual([]);
});

it('right-clicking outside the selected nodes scopes deletion to that node only', () => {
  const f = setup(['a', 'c']);
  act(() => f.result.current.menu.prepare(target('node', 'b')));
  expect(f.result.current.nodes.filter((n) => n.selected).map((n) => n.id)).toEqual(['b']);
  act(() => f.result.current.menu.execute('context:remove'));
  expect(f.result.current.current.nodes.map((n) => n.id)).toEqual(['a', 'c']);
});

it('deletes selected cables without removing their nodes', () => {
  const f = setup([], ['ab', 'cb']);
  act(() => f.result.current.menu.prepare(target('edge', 'ab')));
  act(() => f.result.current.menu.execute('context:remove'));
  expect(f.result.current.current.nodes).toEqual(model.nodes);
  expect(f.result.current.current.edges).toEqual([]);
});

it('disconnects node cables while preserving all nodes and unrelated edges', () => {
  const f = setup();
  act(() => f.result.current.menu.prepare(target('node', 'a')));
  act(() => f.result.current.menu.execute('context:disconnect'));
  expect(f.result.current.current.nodes).toEqual(model.nodes);
  expect(f.result.current.current.edges.map((e) => e.id)).toEqual(['cb']);
});

it.each(['input', 'output'] as const)('disconnects the exact %s channel', (side) => {
  const f = setup();
  act(() => f.result.current.menu.prepare(target('port', side === 'input' ? 'b' : 'a', side)));
  act(() => f.result.current.menu.execute('context:disconnect'));
  expect(f.result.current.current.edges.map((e) => e.id)).toEqual(side === 'input' ? [] : ['cb']);
});

it('keeps the selected port group as the exact execution and highlight set', () => {
  const ports: Port[] = ['a', 'c'].map((nodeId) => ({ nodeId, portId: 'out', side: 'output' }));
  const f = setup(['b'], [], ports);
  act(() => f.result.current.menu.prepare(target('port', 'a')));
  expect(f.selectPorts).toHaveBeenCalledWith(ports);
  expect(f.result.current.nodes.some((n) => n.selected)).toBe(false);
  act(() => f.result.current.menu.execute('context:disconnect'));
  expect(f.result.current.current.edges).toEqual([]);
  expect(f.committed).toHaveBeenCalledTimes(1);
});

it('replaces old port highlights when right-clicking an unselected port', () => {
  const f = setup(['b'], [], [{ nodeId: 'c', portId: 'out', side: 'output' }]);
  act(() => f.result.current.menu.prepare(target('port', 'a')));
  expect(f.selectPorts).toHaveBeenCalledWith([{ nodeId: 'a', portId: 'out', side: 'output' }]);
  act(() => f.result.current.menu.execute('context:disconnect'));
  expect(f.result.current.current.edges.map((e) => e.id)).toEqual(['cb']);
});

it('guards disabled commands and leaves node-add ids to the creation handler', () => {
  const f = setup();
  act(() => f.result.current.menu.prepare(target('port', 'c', 'input')));
  expect(f.result.current.menu.items[0]?.disabled).toBe(true);
  act(() => f.result.current.menu.execute('context:disconnect'));
  expect(f.committed).not.toHaveBeenCalled();
  expect(f.result.current.menu.execute('source')).toBe(false);
});

it('selects all, clears all and fits the viewport without recording a graph mutation', () => {
  const f = setup();
  act(() => f.result.current.menu.prepare(target('canvas')));
  act(() => f.result.current.menu.execute('context:select'));
  expect(f.result.current.nodes.every((n) => n.selected)).toBe(true);
  expect(f.result.current.edges.every((e) => e.selected)).toBe(true);
  act(() => f.result.current.menu.prepare(target('canvas')));
  act(() => f.result.current.menu.execute('context:clear'));
  act(() => f.result.current.menu.execute('context:fit'));
  expect(f.result.current.nodes.some((n) => n.selected)).toBe(false);
  expect(f.result.current.edges.some((e) => e.selected)).toBe(false);
  expect(f.fitView).toHaveBeenCalledOnce();
  expect(f.committed).not.toHaveBeenCalled();
});

it('right-clicking an unselected cable replaces the current node selection before deleting only that cable', () => {
  const f = setup(['a'], ['ab']);
  act(() => f.result.current.menu.prepare(target('edge', 'cb')));
  expect(f.result.current.edges.filter((edge) => edge.selected).map((edge) => edge.id)).toEqual([
    'cb',
  ]);
  expect(f.result.current.nodes.some((node) => node.selected)).toBe(false);
  act(() => f.result.current.menu.execute('context:remove'));
  expect(f.result.current.current.edges.map((edge) => edge.id)).toEqual(['ab']);
});

it('ignores non-element events and unknown built-in action IDs without affecting the existing selection', () => {
  const f = setup(['a']);
  act(() => f.result.current.menu.prepare({ target: null }));
  expect(f.result.current.menu.execute('context:unknown')).toBe(true);
  expect(f.result.current.nodes.filter((node) => node.selected).map((node) => node.id)).toEqual([
    'a',
  ]);
  expect(f.committed).not.toHaveBeenCalled();
});

it('clears node, cable and port selections without mutating the graph', () => {
  const f = setup(['a'], ['ab'], [{ nodeId: 'c', portId: 'out', side: 'output' }]);
  act(() => f.result.current.menu.clearSelection());
  expect(f.result.current.nodes.every((node) => !node.selected)).toBe(true);
  expect(f.result.current.edges.every((edge) => !edge.selected)).toBe(true);
  expect(f.result.current.selectedPorts.current).toEqual([]);
  expect(f.committed).not.toHaveBeenCalled();
});

it.each(['missing-node', 'missing-port', 'invalid-side'] as const)(
  'treats an incomplete %s port target as canvas instead of disconnecting an unrelated channel',
  (invalid) => {
    const f = setup();
    const event = target('port');
    if (invalid === 'missing-node') delete event.target.dataset.audioNodeId;
    if (invalid === 'missing-port') delete event.target.dataset.audioPortId;
    if (invalid === 'invalid-side') event.target.dataset.audioPortSide = 'invalid';
    act(() => f.result.current.menu.prepare(event));
    expect(f.result.current.menu.items.some((item) => item.id === 'context:fit')).toBe(true);
    act(() => f.result.current.menu.execute('context:disconnect'));
    expect(f.committed).not.toHaveBeenCalled();
  },
);

it('preserves the entire selected group when its selected node opens a context menu', () => {
  const f = setup(['a', 'c'], ['cb']);
  act(() => f.result.current.menu.prepare(target('node', 'a')));
  expect(f.result.current.nodes.filter((node) => node.selected).map((node) => node.id)).toEqual([
    'a',
    'c',
  ]);
  act(() => f.result.current.menu.execute('context:remove'));
  expect(f.result.current.current.nodes.map((node) => node.id)).toEqual(['b']);
  expect(f.result.current.current.edges).toEqual([]);
});

it('offers rename only for one node and delegates without editing the audio graph', () => {
  const rename = vi.fn();
  const f = setup([], [], [], { renameNode: rename });
  act(() => f.result.current.menu.prepare(target('node', 'a')));
  expect(f.result.current.menu.items[0]).toMatchObject({
    id: 'context:rename',
    label: 'Rename display name',
    description: 'F2',
  });
  act(() => f.result.current.menu.execute('context:rename'));
  expect(rename).toHaveBeenCalledExactlyOnceWith('a');
  expect(f.committed).not.toHaveBeenCalled();
  expect(f.result.current.current).toBe(model);
  const multiple = setup(['a', 'b'], [], [], { renameNode: rename });
  act(() => multiple.result.current.menu.prepare(target('node', 'a')));
  expect(multiple.result.current.menu.items.some((item) => item.id === 'context:rename')).toBe(
    false,
  );
  const named = setup([], [], [], {
    renameNode: rename,
    labels: {
      renameNode: '표시 이름 변경',
      removeNode: 'Delete',
      removeSelection: 'Delete selection',
      disconnect: 'Disconnect',
      disconnectPort: 'Disconnect port',
      disconnectPorts: 'Disconnect ports',
      removeCable: 'Delete cable',
      fitView: 'Fit',
      selectAll: 'All',
      clearSelection: 'Clear',
    },
  });
  act(() => named.result.current.menu.prepare(target('node', 'b')));
  expect(named.result.current.menu.items[0]?.label).toBe('표시 이름 변경');
});

it('node-specific control actions are available only for one node and dispatch without graph edits', () => {
  const assign = vi.fn();
  const f = setup([], [], [], {
    nodeActions: (nodeId) => [
      { id: 'midi:mute', label: `Assign MIDI: ${nodeId}`, onAction: assign },
    ],
  });
  act(() => f.result.current.menu.prepare(target('node', 'a')));
  expect(f.result.current.menu.items.some((item) => item.id === 'midi:mute')).toBe(true);
  act(() => expect(f.result.current.menu.execute('midi:mute')).toBe(true));
  expect(assign).toHaveBeenCalledTimes(1);
  expect(f.committed).not.toHaveBeenCalled();
  act(() => f.result.current.menu.prepare(target('canvas')));
  expect(f.result.current.menu.items.some((item) => item.id === 'midi:mute')).toBe(false);
});
