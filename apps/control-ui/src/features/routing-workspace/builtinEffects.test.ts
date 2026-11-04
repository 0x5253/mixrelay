import { expect, it } from 'vitest';
import { type AudioGraphModel, graphNodeDefaultWidth } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { effectTranslations } from '../../core/i18n/effects';
import {
  chainKinds,
  effectControls,
  effectDefaults,
  effectKinds,
  effectPresets,
} from './builtinEffects';
import { addRoutingNode } from './routingNodeCreation';
import { updateRoutingNodeParameter } from './routingNodeParameters';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';

const empty: AudioGraphModel = { revision: 10, nodes: [], edges: [] };
const document: RuntimeGraphDocument = { schemaVersion: 2, revision: 10, nodes: [], edges: [] };

it.each(effectKinds)('persists %s controls, bypass and channel geometry across restore', (kind) => {
  let model = addRoutingNode(empty, { kind: 'effect', effect: kind }, { x: 10, y: 20 });
  const node = model.nodes[0];
  if (!node) throw new Error('Effect was not added');
  expect(node.position).toEqual({ x: 10, y: 20 });
  expect(audioGraphToRuntimeGraph(model, document).nodes[0]?.config).toMatchObject({
    bypass: false,
  });
  expect(node.inputPorts).toHaveLength(kind === 'pan' ? 1 : kind === 'ducking' ? 3 : 2);
  expect(node.outputPorts).toHaveLength(2);
  model = updateRoutingNodeParameter(model, node.id, 'bypass', 1);
  const saved = audioGraphToRuntimeGraph(model, document);
  expect(saved.nodes[0]?.config).toEqual({
    version: 1,
    channels: kind === 'pan' ? 1 : 2,
    bypass: true,
    effect: { kind, parameters: effectDefaults[kind] },
  });
  const restored = runtimeGraphToAudioGraph(
    JSON.parse(JSON.stringify(saved)) as RuntimeGraphDocument,
  );
  expect(restored.nodes[0]?.appearance.parameterControls).toEqual(
    model.nodes[0]?.appearance.parameterControls,
  );
});

it.each(chainKinds)('adds %s as a connected, editable chain in one graph revision', (preset) => {
  const graph = addRoutingNode(empty, { kind: 'effect_chain', preset }, { x: 0, y: 0 });
  expect(graph.revision).toBe(11);
  expect(graph.nodes.length).toBeGreaterThanOrEqual(2);
  expect(graph.edges).toHaveLength((graph.nodes.length - 1) * 2);
  expect(new Set(graph.nodes.map((node) => node.id)).size).toBe(graph.nodes.length);
  const saved = audioGraphToRuntimeGraph(graph, document);
  expect(saved.edges.every((edge) => edge.channelMapping?.mode === 'explicit')).toBe(true);
  expect(runtimeGraphToAudioGraph(saved).nodes).toHaveLength(graph.nodes.length);
});

it('applies presets atomically, returns to custom after editing and preserves localized choices', () => {
  let graph = addRoutingNode(empty, { kind: 'effect', effect: 'reverb' }, { x: 0, y: 0 });
  const id = graph.nodes[0]?.id;
  if (!id) throw new Error('Reverb was not added');
  graph = updateRoutingNodeParameter(graph, id, 'preset', 3);
  expect(graph.revision).toBe(12);
  expect(
    graph.nodes[0]?.appearance.parameterControls?.find((control) => control.id === 'decaySeconds')
      ?.value,
  ).toBe(3);
  graph = updateRoutingNodeParameter(graph, id, 'mix', 0.41);
  expect(
    graph.nodes[0]?.appearance.parameterControls?.find((control) => control.id === 'preset')?.value,
  ).toBe(0);
  expect(updateRoutingNodeParameter(graph, id, 'mix', NaN)).toBe(graph);
  const keys = Object.keys(effectTranslations.en);
  for (const labels of Object.values(effectTranslations)) {
    expect(Object.keys(labels)).toEqual(keys);
    expect(Object.values(labels).every((label) => Boolean(label.trim()))).toBe(true);
  }
});

it('preserves preset identity when the same decimal value is entered again', () => {
  const added = addRoutingNode(empty, { kind: 'effect', effect: 'reverb' }, { x: 0, y: 0 });
  const id = added.nodes[0]?.id ?? '';
  const hall = updateRoutingNodeParameter(added, id, 'preset', 3);
  expect(updateRoutingNodeParameter(hall, id, 'damping', 0.35)).toBe(hall);
  const changed = updateRoutingNodeParameter(hall, id, 'damping', 0.45);
  expect(
    changed.nodes[0]?.appearance.parameterControls?.find((c) => c.id === 'damping')?.value,
  ).toBe(0.45);
});

it('uses the same minimum-based step grid as Core numeric and knob controls', () => {
  const added = addRoutingNode(empty, { kind: 'compressor' }, { x: 0, y: 0 });
  const id = added.nodes[0]?.id ?? '';
  const changed = updateRoutingNodeParameter(added, id, 'attackMs', 10.6);
  expect(
    changed.nodes[0]?.appearance.parameterControls?.find((c) => c.id === 'attackMs')?.value,
  ).toBe(10.6);
});

it.each(['reverb', 'delay'] as const)(
  'persists every %s preset with its actual spatial geometry or echo mode',
  (kind) => {
    const added = addRoutingNode(empty, { kind: 'effect', effect: kind }, { x: 0, y: 0 });
    const id = added.nodes[0]?.id ?? '';
    for (const [index, preset] of (effectPresets[kind] ?? []).entries()) {
      const applied = updateRoutingNodeParameter(added, id, 'preset', index + 1);
      const saved = audioGraphToRuntimeGraph(applied, document);
      expect(saved.nodes[0]?.config).toEqual({
        version: 1,
        channels: 2,
        bypass: false,
        effect: { kind, parameters: preset.parameters },
      });
      const restored = runtimeGraphToAudioGraph(
        JSON.parse(JSON.stringify(saved)) as RuntimeGraphDocument,
      );
      expect(
        restored.nodes[0]?.appearance.parameterControls?.find((c) => c.id === 'preset')?.value,
      ).toBe(index + 1);
      const changed = updateRoutingNodeParameter(
        restored,
        id,
        'mix',
        preset.parameters.mix === 1 ? 0 : 1,
      );
      expect(
        changed.nodes[0]?.appearance.parameterControls?.find((c) => c.id === 'preset')?.value,
      ).toBe(0);
    }
  },
);

it('keeps three primary reverb knobs and exposes real algorithms and geometry only through shared Options', () => {
  const controls = effectControls('reverb', effectDefaults.reverb, false, effectTranslations.en);
  expect(controls.filter((c) => c.section === 'main' && !c.choices).map((c) => c.id)).toEqual([
    'decaySeconds',
    'damping',
    'mix',
  ]);
  expect(controls.filter((c) => c.section === 'options').map((c) => c.id)).toEqual([
    'roomSize',
    'preDelayMs',
    'algorithm',
  ]);
  expect(controls.find((c) => c.id === 'algorithm')?.choices).toEqual([
    { value: 0, label: 'Classic' },
    { value: 1, label: 'Dense' },
  ]);
  const added = addRoutingNode(empty, { kind: 'effect', effect: 'reverb' }, { x: 0, y: 0 });
  const id = added.nodes[0]?.id ?? '';
  const hall = updateRoutingNodeParameter(added, id, 'preset', 3);
  const edited = updateRoutingNodeParameter(hall, id, 'preDelayMs', 40);
  const saved = audioGraphToRuntimeGraph(edited, document);
  expect(saved.nodes[0]?.config).toMatchObject({
    effect: { parameters: { preDelayMs: 40, roomSize: 1.5, algorithm: 1 } },
  });
  expect(edited.nodes[0]?.appearance.parameterControls?.find((c) => c.id === 'preset')?.value).toBe(
    0,
  );
});

it('limits echo modes to stereo or actual ping-pong independently from channel utility modes', () => {
  const controls = effectControls('delay', effectDefaults.delay, false, effectTranslations.en);
  expect(controls.find((c) => c.id === 'mode')).toMatchObject({
    label: 'Delay mode',
    maximum: 1,
    choices: [
      { value: 0, label: 'Stereo echo' },
      { value: 1, label: 'Ping-pong' },
    ],
  });
  const added = addRoutingNode(empty, { kind: 'effect', effect: 'delay' }, { x: 0, y: 0 });
  const changed = updateRoutingNodeParameter(added, added.nodes[0]?.id ?? '', 'mode', 5);
  expect(audioGraphToRuntimeGraph(changed, document).nodes[0]?.config).toMatchObject({
    effect: { parameters: { mode: 1 } },
  });
});

it.each([
  ['karaoke', ['delay', 'reverb']],
  ['tunnel', ['equalizer', 'delay', 'reverb']],
  ['fishbowl', ['high_pass', 'low_pass', 'chorus', 'reverb']],
  ['radio', ['high_pass', 'low_pass', 'compressor']],
  ['doubleVocal', ['chorus', 'delay']],
] as const)(
  'inserts %s left to right after existing nodes, wired channel for channel',
  (preset, kinds) => {
    const existing = addRoutingNode(empty, { kind: 'limiter' }, { x: 0, y: 0 });
    const graph = addRoutingNode(existing, { kind: 'effect_chain', preset }, { x: 44, y: 33 });
    const saved = audioGraphToRuntimeGraph(graph, document);
    expect(saved.nodes[0]).toEqual(audioGraphToRuntimeGraph(existing, document).nodes[0]);
    const chain = saved.nodes.slice(1);
    expect(
      chain.map((node) =>
        node.type === 'effect'
          ? (node.config as { effect: { kind: string } }).effect.kind
          : node.type,
      ),
    ).toEqual(kinds);
    const views = graph.nodes.slice(1);
    expect(views[0]?.position).toEqual({ x: 44, y: 33 });
    for (let index = 1; index < chain.length; index += 1) {
      const [previous, next] = [views[index - 1], views[index]];
      if (!previous || !next) throw new Error('The chain is missing a node');
      expect(
        next.position.x - previous.position.x - graphNodeDefaultWidth(previous),
      ).toBeGreaterThan(0);
      const connections = saved.edges.filter(
        (edge) =>
          edge.source.nodeId === chain[index - 1]?.id &&
          edge.destination.nodeId === chain[index]?.id,
      );
      expect(connections.map((edge) => edge.channelMapping)).toEqual([
        { mode: 'explicit', routes: [{ sourceChannel: 0, destinationChannel: 0 }] },
        { mode: 'explicit', routes: [{ sourceChannel: 1, destinationChannel: 1 }] },
      ]);
    }
  },
);

it.each([
  ['lowFrequencyHz', 350],
  ['lowDb', 4.2],
  ['lowQ', 1.234],
  ['midFrequencyHz', 2200],
  ['midDb', -3.1],
  ['midQ', 2.345],
  ['highFrequencyHz', 8800],
  ['highDb', 5.6],
  ['highQ', 3.456],
] as const)('editing EQ %s changes only that value', (id, value) => {
  const graph = addRoutingNode(empty, { kind: 'effect', effect: 'equalizer' }, { x: 0, y: 0 });
  const nodeId = graph.nodes[0]?.id ?? '';
  const edited = audioGraphToRuntimeGraph(
    updateRoutingNodeParameter(graph, nodeId, id, value),
    document,
  );
  expect(edited.nodes[0]?.config).toMatchObject({
    effect: { parameters: { ...effectDefaults.equalizer, [id]: value } },
  });
});

it.each([
  ['en', effectTranslations.en],
  ['ko-KR', effectTranslations.koKR],
] as const)('names every %s channel mode with a description of its result', (_locale, labels) => {
  const mode = effectControls(
    'channel_utility',
    effectDefaults.channel_utility,
    false,
    labels,
  ).find((control) => control.id === 'mode');
  expect(mode?.choices).toEqual(
    (['unchanged', 'mono', 'left', 'right', 'swap', 'invert'] as const).map((key, value) => ({
      value,
      label: labels[key],
      description: labels[`${key}Description`],
    })),
  );
});
