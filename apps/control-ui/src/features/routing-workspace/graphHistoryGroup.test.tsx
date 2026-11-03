import { act, renderHook } from '@testing-library/react';
import { expect, it } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { useUndoHistory } from '../../core/history/useUndoHistory';

import { graphHistoryGroup } from './graphHistoryGroup';
import {
  addRoutingNode,
  runtimeGraphToAudioGraph,
  updateRoutingNodeGain,
  updateRoutingNodeParameter,
  updateRoutingNodeParameters,
} from './model';

it('gives each control on one compressor its own history group', () => {
  const before = addRoutingNode(
    runtimeGraphToAudioGraph({ schemaVersion: 2, revision: 1, nodes: [], edges: [] }),
    { kind: 'compressor' },
    { x: 0, y: 0 },
  );
  const id = before.nodes[0]?.id ?? '';
  const threshold = updateRoutingNodeParameter(before, id, 'thresholdDb', -12);
  const ratio = updateRoutingNodeParameter(threshold, id, 'ratio', 6);
  const thresholdGroup = graphHistoryGroup(before, threshold, 'level');
  const ratioGroup = graphHistoryGroup(threshold, ratio, 'level');
  expect(thresholdGroup).toBeDefined();
  expect(ratioGroup).toBeDefined();
  expect(thresholdGroup).not.toBe(ratioGroup);
});

it('groups a single gain gesture and keeps topology changes out of gesture history', () => {
  const before = runtimeGraphToAudioGraph({
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: [{ id: 'gain', type: 'gain', config: { channels: 2, gainDb: 0 }, label: null }],
  });
  const changed = updateRoutingNodeGain(before, 'gain', -4);
  expect(graphHistoryGroup(before, changed, 'level')).toBe('level:gain:gain');
  expect(graphHistoryGroup(before, changed, 'layout')).toBe('layout:gain');
  expect(graphHistoryGroup(before, before, 'level')).toBeUndefined();
  expect(graphHistoryGroup(before, { ...changed, edges: [] }, 'level')).toBeUndefined();
  expect(graphHistoryGroup(before, { ...changed, nodes: [] }, 'level')).toBeUndefined();
  expect(
    graphHistoryGroup(before, { ...changed, nodes: [...changed.nodes, ...changed.nodes] }, 'level'),
  ).toBeUndefined();
  expect(
    graphHistoryGroup(
      before,
      { ...changed, nodes: changed.nodes.map((node) => ({ ...node, id: 'replacement' })) },
      'level',
    ),
  ).toBeUndefined();
  expect(
    graphHistoryGroup(
      before,
      {
        ...changed,
        nodes: changed.nodes.map((node) => ({
          ...node,
          appearance: { ...node.appearance, gainControl: undefined },
        })),
      },
      'processor_parameter',
    ),
  ).toBe('processor_parameter:gain:gain');
});

it('does not merge a gesture that changed multiple controls or only presentation', () => {
  const before = addRoutingNode(
    runtimeGraphToAudioGraph({ schemaVersion: 2, revision: 1, nodes: [], edges: [] }),
    { kind: 'compressor' },
    { x: 0, y: 0 },
  );
  const id = before.nodes[0]?.id ?? '';
  const changed = updateRoutingNodeParameter(
    updateRoutingNodeParameter(before, id, 'thresholdDb', -12),
    id,
    'ratio',
    6,
  );
  expect(graphHistoryGroup(before, changed, 'processor_parameter')).toBeUndefined();
  const moved = {
    ...before,
    nodes: before.nodes.map((node) => ({ ...node, position: { x: 20, y: 30 } })),
  };
  expect(graphHistoryGroup(before, moved, 'level')).toBeUndefined();
  expect(
    graphHistoryGroup(
      {
        ...before,
        nodes: [...before.nodes, ...before.nodes.map((node) => ({ ...node, id: 'second' }))],
      },
      {
        ...moved,
        nodes: [...moved.nodes, ...moved.nodes.map((node) => ({ ...node, id: 'second' }))],
      },
      'layout',
    ),
  ).toBeUndefined();
});

it('keeps a preset-to-custom parameter gesture in one group and preset selection atomic', () => {
  const empty = runtimeGraphToAudioGraph({ schemaVersion: 2, revision: 1, nodes: [], edges: [] });
  const added = addRoutingNode(empty, { kind: 'effect', effect: 'reverb' }, { x: 0, y: 0 });
  const id = added.nodes[0]?.id ?? '';
  const hall = updateRoutingNodeParameter(added, id, 'preset', 3);
  const first = updateRoutingNodeParameter(hall, id, 'mix', 0.31);
  const second = updateRoutingNodeParameter(first, id, 'mix', 0.32);
  expect(graphHistoryGroup(hall, first, 'level')).toBe(`level:${id}:parameter:mix`);
  expect(graphHistoryGroup(first, second, 'level')).toBe(graphHistoryGroup(hall, first, 'level'));
  expect(graphHistoryGroup(added, hall, 'runtime')).toBeUndefined();
  const matched = updateRoutingNodeParameter(first, id, 'mix', 0.3);
  const unmatched = updateRoutingNodeParameter(matched, id, 'mix', 0.29);
  expect(graphHistoryGroup(first, matched, 'level')).toBe(graphHistoryGroup(hall, first, 'level'));
  expect(graphHistoryGroup(matched, unmatched, 'level')).toBe(
    graphHistoryGroup(hall, first, 'level'),
  );
  const { result } = renderHook(() => useUndoHistory<AudioGraphModel, 'level'>());
  act(() => {
    result.current.recordDebounced(
      hall,
      first,
      'level',
      graphHistoryGroup(hall, first, 'level') ?? '',
    );
    result.current.recordDebounced(
      first,
      second,
      'level',
      graphHistoryGroup(first, second, 'level') ?? '',
    );
  });
  expect(result.current.undo()?.value).toBe(hall);
  expect(result.current.undo()).toBeNull();
});

it('groups both axes of one EQ band drag and separates other bands and numeric fields', () => {
  const before = addRoutingNode(
    runtimeGraphToAudioGraph({ schemaVersion: 2, revision: 1, nodes: [], edges: [] }),
    { kind: 'effect', effect: 'equalizer' },
    { x: 0, y: 0 },
  );
  const id = before.nodes[0]?.id ?? '';
  const first = updateRoutingNodeParameters(before, id, [
    { id: 'lowFrequencyHz', value: 200 },
    { id: 'lowDb', value: 4 },
  ]);
  const second = updateRoutingNodeParameters(first, id, [
    { id: 'lowFrequencyHz', value: 240 },
    { id: 'lowDb', value: 6 },
  ]);
  const third = updateRoutingNodeParameters(second, id, [{ id: 'highDb', value: -2 }]);
  const firstGroup = graphHistoryGroup(before, first, 'parameter_group');
  expect(firstGroup).toBeDefined();
  expect(graphHistoryGroup(first, second, 'parameter_group')).toBe(firstGroup);
  expect(graphHistoryGroup(second, third, 'parameter_group')).not.toBe(firstGroup);
  expect(graphHistoryGroup(before, first, 'level')).toBeUndefined();
  expect(
    graphHistoryGroup(first, updateRoutingNodeParameter(first, id, 'lowDb', 5), 'level'),
  ).not.toBe(firstGroup);
  const { result } = renderHook(() => useUndoHistory<AudioGraphModel, 'parameter_group'>());
  act(() => {
    result.current.recordDebounced(before, first, 'parameter_group', firstGroup ?? '');
    result.current.recordDebounced(first, second, 'parameter_group', firstGroup ?? '');
  });
  expect(result.current.undo()?.value).toBe(before);
  expect(result.current.undo()).toBeNull();
  const mixed = updateRoutingNodeParameters(before, id, [
    { id: 'lowDb', value: 4 },
    { id: 'highDb', value: 6 },
  ]);
  expect(graphHistoryGroup(before, mixed, 'parameter_group')).toBeUndefined();
  const q = updateRoutingNodeParameter(before, id, 'lowQ', 1);
  expect(graphHistoryGroup(before, q, 'parameter_group')).toBeUndefined();
});

it('does not group an unrelated node or presentation refresh as an EQ drag', () => {
  const model = runtimeGraphToAudioGraph({
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: [{ id: 'gain', type: 'gain', config: { channels: 2, gainDb: 0 }, label: null }],
  });
  const moved = {
    ...model,
    nodes: model.nodes.map((node) => ({ ...node, position: { x: 10, y: 20 } })),
  };
  expect(graphHistoryGroup(model, moved, 'parameter_group')).toBeUndefined();
  const eq = addRoutingNode(model, { kind: 'effect', effect: 'equalizer' }, { x: 0, y: 0 });
  const named = {
    ...eq,
    nodes: eq.nodes.map((node) =>
      node.appearance.equalizer
        ? { ...node, appearance: { ...node.appearance, title: 'Renamed EQ' } }
        : node,
    ),
  };
  expect(graphHistoryGroup(eq, named, 'parameter_group')).toBeUndefined();
});
