import { expect, it } from 'vitest';
import {
  acceptedConnections,
  availableChannels,
  type ChannelConnectionChoices,
  channelConnectionForm,
  initialChoices,
  rangeFromSelection,
  rangeStartingAt,
} from './channelConnectionForm';
import type { AudioGraphModel, AudioGraphNode } from './model';

function node(id: string, title: string, inputs: number, outputs: number): AudioGraphNode {
  const ports = (prefix: string, label: string, count: number) =>
    Array.from({ length: count }, (_, index) => ({ id: `${prefix}${index + 1}`, label }));
  return {
    id,
    position: { x: 0, y: 0 },
    inputPorts: ports('in', `${title} input`, inputs),
    outputPorts: ports('out', `${title} output`, outputs),
    appearance: { category: 'Test', detail: id, icon: 'T', title },
  };
}

function graph(): AudioGraphModel {
  return {
    revision: 1,
    nodes: [
      node('app', 'Music', 0, 4),
      node('mon-a', 'Monitor', 2, 0),
      node('mon-b', 'Monitor', 3, 0),
      node('meter', 'Meter', 0, 2),
    ],
    edges: [],
  };
}

const form = (
  choices: Partial<ChannelConnectionChoices>,
  model = graph(),
  sourcePortIds?: readonly string[],
  sourceNodeId = 'app',
) =>
  channelConnectionForm(model, sourceNodeId, sourcePortIds, {
    ...initialChoices(model, sourceNodeId, sourcePortIds),
    ...choices,
  });

const pairs = (connections: readonly { sourcePortId: string; targetPortId: string }[]) =>
  connections.map(({ sourcePortId, targetPortId }) => `${sourcePortId}->${targetPortId}`);

it('starts from every output and plans nothing until a destination is chosen', () => {
  const start = form({});
  expect(start.sourceRange).toEqual({ startPortId: 'out1', count: 4 });
  expect(start.request).toBeNull();
  expect(start.plan).toEqual({ accepted: [], skipped: [] });
  expect(acceptedConnections(graph(), start.request)).toEqual([]);
});

it('offers only nodes with inputs and names repeated titles distinctly', () => {
  expect(form({}).targets).toEqual([
    { value: 'mon-a', label: 'Monitor (1)' },
    { value: 'mon-b', label: 'Monitor (2)' },
  ]);
});

it('plans each channel from the chosen first input and skips the ones without a match', () => {
  const planned = form({ targetNodeId: 'mon-b', targetStartPortId: 'in2' });
  expect(pairs(planned.plan.accepted)).toEqual(['out1->in2', 'out2->in3']);
  expect(planned.plan.skipped.map(({ sourcePortId }) => sourcePortId)).toEqual(['out3', 'out4']);
  // Without a choice the first input is assumed.
  expect(form({ targetNodeId: 'mon-b' }).targetAnchorPortId).toBe('in1');
});

it('clamps the channel count to the outputs left after a later first channel', () => {
  const outputs = form({}).outputs;
  expect(rangeStartingAt(outputs, { startPortId: 'out1', count: 4 }, 'out3')).toEqual({
    startPortId: 'out3',
    count: 2,
  });
  expect(availableChannels(outputs, { startPortId: 'out3', count: 2 })).toBe(2);
});

it('keeps a sparse canvas selection until a covering range is chosen', () => {
  const kept = form({ targetNodeId: 'mon-b' }, graph(), ['out2', 'out4']);
  expect(kept.sourceRange).toBeNull();
  expect(kept.selection.map(({ port }) => port.id)).toEqual(['out2', 'out4']);
  expect(pairs(kept.plan.accepted)).toEqual(['out2->in1', 'out4->in3']);
  expect(rangeFromSelection(kept.outputs, kept.selection)).toEqual({
    startPortId: 'out2',
    count: 3,
  });
});

it('submits only what the graph accepts at the moment of submission', () => {
  const planned = form({ targetNodeId: 'mon-a', range: { startPortId: 'out1', count: 2 } });
  const changed = graph();
  changed.edges = [
    {
      id: 'e',
      sourceNodeId: 'app',
      sourcePortId: 'out1',
      targetNodeId: 'mon-a',
      targetPortId: 'in1',
    },
  ];
  expect(pairs(acceptedConnections(changed, planned.request))).toEqual(['out2->in2']);
});

it('asks again for an input channel that disappeared instead of moving the cables', () => {
  const shrunk = graph();
  shrunk.nodes[2] = node('mon-b', 'Monitor', 2, 0);
  const asked = form({ targetNodeId: 'mon-b', targetStartPortId: 'in3' }, shrunk);
  expect(asked.targetAnchorPortId).toBeNull();
  expect(asked.request).toBeNull();
});

it('leaves a vanished first output unselected, with no channels to cover', () => {
  const shrunk = graph();
  shrunk.nodes[0] = node('app', 'Music', 0, 2);
  const vanished = form(
    { targetNodeId: 'mon-a', range: { startPortId: 'out4', count: 1 } },
    shrunk,
  );
  expect(availableChannels(vanished.outputs, { startPortId: 'out4', count: 1 })).toBe(0);
  expect(vanished.request).toBeNull();
});

it('falls back to choosing a range when every selected channel disappeared', () => {
  const shrunk = graph();
  shrunk.nodes[0] = node('app', 'Music', 0, 2);
  const fallback = channelConnectionForm(shrunk, 'app', ['out4'], {
    range: null,
    targetNodeId: null,
    targetStartPortId: null,
  });
  expect(fallback.selection).toEqual([]);
  expect(fallback.sourceRange).toEqual({ startPortId: '', count: 1 });
});

it('offers a boundary self-connection that dragging allows, but no other self-loop', () => {
  const device = node('virtual', 'Virtual device', 1, 1);
  device.inputPorts[0] = { boundary: 'sink', id: 'to_os', label: 'To system' };
  device.outputPorts[0] = { boundary: 'source', id: 'from_os', label: 'From system' };
  const model: AudioGraphModel = {
    revision: 1,
    nodes: [device, node('fx', 'Effect', 1, 1)],
    edges: [],
  };
  const self = form({ targetNodeId: 'virtual' }, model, undefined, 'virtual');
  expect(self.targets.map(({ value }) => value)).toEqual(['virtual', 'fx']);
  expect(pairs(acceptedConnections(model, self.request))).toEqual(['from_os->to_os']);
  expect(form({}, model, undefined, 'fx').targets.map(({ value }) => value)).toEqual(['virtual']);
});

it('plans nothing once the source leaves the graph while the form is open', () => {
  const without = { ...graph(), nodes: graph().nodes.slice(1) };
  expect(
    form({ targetNodeId: 'mon-a', range: { startPortId: 'out1', count: 4 } }, without).request,
  ).toBeNull();
});

it('has no channels or request for a source that is not in the graph', () => {
  const gone = form({ targetNodeId: 'mon-a' }, graph(), undefined, 'gone');
  expect(gone.sourceTitle).toBe('gone');
  expect(form({}).sourceTitle).toBe('Music');
  expect(gone.outputs).toEqual([]);
  expect(gone.request).toBeNull();
});
