import { expect, it, vi } from 'vitest';
import { UndoHistory } from '../../core/application/history/UndoHistory';
import type { AudioGraphChangeKind } from '../../core/audio-graph/AudioGraphCanvas';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { fakeScheduler } from '../../test-support/fakeScheduler';
import { effectPresets } from './builtinEffects';
import { GraphEditSession, type GraphEditTargets } from './GraphEditSession';
import type { RoutingNodeRequest } from './nodeRequests';
import { addRoutingNode } from './routingNodeCreation';
import {
  parameterChangeKind,
  updateRoutingNodeParameter,
  updateRoutingNodeParameters,
} from './routingNodeParameters';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';

// The edits RoutingWorkspace makes for canvas callbacks, run through a real GraphEditSession
// and UndoHistory, with the engine reduced to the documents it would be sent.
const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
const IDLE = 1_000;

function workspace(...requests: RoutingNodeRequest[]) {
  const clock = fakeScheduler();
  const initial = requests.reduce(
    (model, request) => addRoutingNode(model, request, { x: 0, y: 0 }),
    runtimeGraphToAudioGraph(empty),
  );
  const model = { current: initial as AudioGraphModel | null };
  const submit = vi.fn<(model: AudioGraphModel) => void>();
  const targets: GraphEditTargets = {
    history: new UndoHistory<AudioGraphModel, AudioGraphChangeKind>(clock, clock),
    queue: { canPublishParameters: false, submit },
    publishParameter: vi.fn(),
    graphRevision: () => 1,
    show: vi.fn(),
    savePositions: vi.fn(),
  };
  const session = new GraphEditSession(model);
  const owner = {};
  session.render(owner, targets);
  session.open(owner);
  const current = () => model.current as AudioGraphModel;
  const nodeId = () => current().nodes[0]?.id ?? '';
  return {
    nodeId,
    /** The document the engine was last asked to apply. */
    submitted: () => audioGraphToRuntimeGraph(submit.mock.lastCall?.[0] as AudioGraphModel, empty),
    config: () =>
      audioGraphToRuntimeGraph(current(), empty).nodes[0]?.config as {
        bypass: boolean;
        effect: { parameters: Record<string, number> };
      } & Record<string, unknown>,
    add(request: RoutingNodeRequest) {
      session.edit(owner, addRoutingNode(current(), request, { x: 0, y: 0 }), 'runtime');
    },
    parameter(id: string, value: number) {
      const next = updateRoutingNodeParameter(current(), nodeId(), id, value);
      session.edit(owner, next, parameterChangeKind(id));
    },
    band(updates: { id: string; value: number }[]) {
      const next = updateRoutingNodeParameters(current(), nodeId(), updates);
      session.edit(owner, next, 'parameter_group');
    },
    idle: () => clock.advance(IDLE),
    undo: () => session.undo(),
    redo: () => session.redo(),
    nodes: () => audioGraphToRuntimeGraph(current(), empty).nodes,
    edges: () => audioGraphToRuntimeGraph(current(), empty).edges,
  };
}

it.each([
  ['processor_parameter', 'external:42'],
  ['runtime', 'preset'],
  ['level', 'decaySeconds'],
  ['level', 'bypass'],
] as const)('records a %s edit for parameter %s', (kind, id) => {
  expect(parameterChangeKind(id)).toBe(kind);
});

it.each(['reverb', 'chorus', 'delay'] as const)(
  'applies a %s preset atomically and keeps the following knob edit a separate Undo step',
  (effect) => {
    const edits = workspace({ kind: 'effect', effect });
    const initial = edits.nodes();
    const preset = effectPresets[effect]?.[0];
    if (!preset) throw new Error(`${effect} has no preset`);
    edits.parameter('preset', 1);
    const selected = edits.nodes();
    expect(edits.submitted().nodes).toEqual(selected);
    expect(edits.config()).toMatchObject({
      bypass: false,
      effect: { parameters: preset.parameters },
    });
    expect(edits.config()).not.toHaveProperty('preset');
    const [knob, value] = Object.entries(preset.parameters)[0] as [string, number];
    edits.parameter(knob, value * 2);
    edits.idle();
    expect(edits.config().effect.parameters).toEqual({ ...preset.parameters, [knob]: value * 2 });
    edits.undo();
    expect(edits.nodes()).toEqual(selected);
    edits.undo();
    expect(edits.nodes()).toEqual(initial);
    edits.redo();
    expect(edits.submitted().nodes).toEqual(selected);
  },
);

it('inserts an effect chain beside existing nodes and undoes the whole chain in one step', () => {
  const edits = workspace({ kind: 'limiter' });
  const existing = edits.nodes();
  edits.add({ kind: 'effect_chain', preset: 'karaoke' });
  const inserted = { nodes: edits.nodes(), edges: edits.edges() };
  expect(inserted.nodes[0]).toEqual(existing[0]);
  expect(inserted.nodes.slice(1).map((node) => node.type)).toEqual(['effect', 'effect']);
  expect(inserted.edges).toHaveLength(2);
  edits.undo();
  expect({ nodes: edits.nodes(), edges: edits.edges() }).toEqual({ nodes: existing, edges: [] });
  edits.redo();
  expect({ nodes: edits.nodes(), edges: edits.edges() }).toEqual(inserted);
});

it('undoes a channel mode choice on its own and redoes it', () => {
  const edits = workspace({ kind: 'effect', effect: 'channel_utility' });
  const original = edits.nodes();
  edits.parameter('mode', 2);
  edits.idle();
  const selected = edits.nodes();
  expect(edits.config().effect.parameters.mode).toBe(2);
  edits.undo();
  expect(edits.submitted().nodes).toEqual(original);
  edits.redo();
  expect(edits.submitted().nodes).toEqual(selected);
});

it('edits one EQ value without touching the other eight, as one Undo step', () => {
  const edits = workspace({ kind: 'effect', effect: 'equalizer' });
  const original = edits.nodes();
  const before = edits.config().effect.parameters;
  edits.parameter('lowFrequencyHz', 350);
  expect(edits.config().effect.parameters).toEqual({ ...before, lowFrequencyHz: 350 });
  edits.undo();
  expect(edits.nodes()).toEqual(original);
});

it('coalesces a continuing EQ band drag of frequency and gain into one Undo step', () => {
  const edits = workspace({ kind: 'effect', effect: 'equalizer' });
  const original = edits.nodes();
  const before = edits.config().effect.parameters;
  edits.band([
    { id: 'midFrequencyHz', value: 1500 },
    { id: 'midDb', value: 3 },
  ]);
  edits.band([
    { id: 'midFrequencyHz', value: 1800 },
    { id: 'midDb', value: 4.5 },
  ]);
  edits.idle();
  const moved = edits.nodes();
  expect(edits.config().effect.parameters).toEqual({
    ...before,
    midFrequencyHz: 1800,
    midDb: 4.5,
  });
  edits.undo();
  expect(edits.nodes()).toEqual(original);
  edits.redo();
  expect(edits.nodes()).toEqual(moved);
});

it('keeps each EQ number gesture its own Undo step while selecting another value', () => {
  const edits = workspace({ kind: 'effect', effect: 'equalizer' });
  const original = edits.nodes();
  edits.parameter('highQ', 0.712);
  edits.parameter('highQ', 0.713);
  edits.idle();
  const dragged = edits.nodes();
  edits.parameter('midQ', 0.71);
  edits.idle();
  expect(edits.config().effect.parameters).toMatchObject({ highQ: 0.713, midQ: 0.71 });
  edits.undo();
  expect(edits.nodes()).toEqual(dragged);
  edits.undo();
  expect(edits.nodes()).toEqual(original);
});

it.each([
  ['Equalizer', { kind: 'effect', effect: 'equalizer' }],
  ['Compressor', { kind: 'compressor' }],
  ['Limiter', { kind: 'limiter' }],
] as const)('toggles %s Bypass without changing its DSP settings', (_name, request) => {
  const edits = workspace(request);
  const original = edits.nodes()[0];
  edits.parameter('bypass', 1);
  expect(edits.submitted().nodes[0]).toEqual({
    ...original,
    config: { ...(original?.config as object), bypass: true },
  });
  edits.parameter('bypass', 0);
  expect(edits.submitted().nodes[0]).toEqual(original);
});
