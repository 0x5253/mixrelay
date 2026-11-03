import { act, renderHook } from '@testing-library/react';
import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { useUndoHistory } from '../../core/history/useUndoHistory';
import { effectLabelsFromWords, effectTranslations } from '../../core/i18n/effects';
import {
  defaultEffectLabels,
  type EffectLabels,
  effectConfig,
  effectControls,
  effectDefaults,
  effectKinds,
  effectParameters,
  effectPresets,
  isChainKind,
  isEffectKind,
} from './builtinEffects';
import {
  addRoutingNode,
  audioGraphToRuntimeGraph,
  type GraphPresentation,
  presentRoutingApplicationIcons,
  runtimeGraphToAudioGraph,
  updateRoutingNodeParameter,
  updateRoutingNodeParameters,
} from './model';
import { duplexReference, virtualSpec } from './virtualEndpointFixtures';

const document: RuntimeGraphDocument = { schemaVersion: 2, revision: 4, nodes: [], edges: [] };
const empty: AudioGraphModel = { revision: 4, nodes: [], edges: [] };
const position = { x: 30, y: 40 };

it.each(['compressor', 'limiter'] as const)(
  'fills %s dynamics and bypass labels missing from a partial presentation without replacing translated labels',
  (kind) => {
    const partialLabels = presentation({ equalizer: '이퀄라이저' });
    const graph = addRoutingNode(empty, { kind }, position, partialLabels);
    const node = first(graph);
    expect(node.appearance.dynamics).toMatchObject({
      inputLabel: 'Input',
      outputLabel: 'Output',
      reductionLabel: 'Gain reduction',
      unavailableLabel: 'Waiting for audio telemetry',
    });
    expect(node.appearance.parameterControls?.find((control) => control.id === 'bypass')).toEqual({
      id: 'bypass',
      label: 'Bypass',
      value: 0,
      minimum: 0,
      maximum: 1,
      step: 1,
      suffix: '',
      choices: [
        { value: 0, label: 'Processing' },
        { value: 1, label: 'Bypassed' },
      ],
    });
    const partial = presentation({ dynamicsInput: '입력', bypass: '바이패스' });
    const restored = runtimeGraphToAudioGraph(audioGraphToRuntimeGraph(graph, document), partial);
    expect(first(restored).appearance.dynamics).toMatchObject({
      inputLabel: '입력',
      outputLabel: 'Output',
      reductionLabel: 'Gain reduction',
      unavailableLabel: 'Waiting for audio telemetry',
    });
    expect(
      first(restored).appearance.parameterControls?.find((control) => control.id === 'bypass'),
    ).toMatchObject({
      label: '바이패스',
      choices: [
        { value: 0, label: 'Processing' },
        { value: 1, label: 'Bypassed' },
      ],
    });
    expect(first(restored).runtime?.config).toEqual(node.runtime?.config);
  },
);

function first(model: AudioGraphModel): AudioGraphNode {
  const node = model.nodes[0];
  if (!node) throw new Error('Expected a built-in effect');
  return node;
}
function presentation(effectLabels: EffectLabels): GraphPresentation {
  return {
    effectLabels,
    application: 'Application',
    audioInput: 'Input',
    audioOutput: 'Output',
    recorder: 'Recorder',
    channels: 'Channels',
    ceiling: 'Ceiling',
    compressor: 'Compressor',
    deviceName: 'Device',
    virtualSourcePort: 'Device source',
    gain: 'Gain',
    mixer: 'Mixer',
    limiter: 'Limiter',
    attack: 'Attack',
    makeupGain: 'Makeup',
    options: 'Options',
    outputGain: 'Output gain',
    processor: 'Processor',
    processorLayoutUnavailable: 'Unavailable',
    providerBusKinds: { main: 'Main', sidechain: 'Control', auxiliary: 'Auxiliary' },
    ratio: 'Ratio',
    release: 'Release',
    routing: 'Routing',
    sourceGain: 'Source gain',
    virtualSinkPort: 'Device sink',
    threshold: 'Threshold',
    unsupported: 'Unsupported',
    utility: 'Utility',
    virtualInput: 'Virtual input',
    virtualOutput: 'Virtual output',
  };
}

it.each(['compressor', 'limiter'] as const)(
  'binds %s to native dynamics telemetry and preserves boolean bypass through storage and one Undo',
  (kind) => {
    const localized = presentation(effectTranslations.koKR);
    const added = addRoutingNode(empty, { kind }, position, localized);
    const node = first(added);
    expect(node.appearance.dynamics).toEqual({
      channels: 2,
      nodeId: node.id,
      portId: 'audio',
      thresholdId: kind === 'compressor' ? 'thresholdDb' : 'ceilingDb',
      inputLabel: effectTranslations.koKR.dynamicsInput,
      outputLabel: effectTranslations.koKR.dynamicsOutput,
      reductionLabel: effectTranslations.koKR.gainReduction,
      unavailableLabel: effectTranslations.koKR.dynamicsUnavailable,
    });
    const changed = updateRoutingNodeParameter(added, node.id, 'bypass', 1);
    expect(first(changed).runtime?.config).toMatchObject({ bypass: true });
    expect(
      first(changed).appearance.parameterControls?.find((control) => control.id === 'bypass'),
    ).toMatchObject({ value: 1, label: effectTranslations.koKR.bypass });
    const serialized = audioGraphToRuntimeGraph(changed, document);
    const restored = runtimeGraphToAudioGraph(serialized, localized);
    expect(first(restored).runtime?.config).toMatchObject({ bypass: true });
    expect(first(restored).appearance.dynamics).toEqual(node.appearance.dynamics);
    const history = renderHook(() => useUndoHistory<AudioGraphModel, 'runtime'>());
    act(() => history.result.current.record(added, changed, 'runtime'));
    let undo: AudioGraphModel | undefined;
    act(() => {
      undo = history.result.current.undo()?.value;
    });
    expect(undo).toBe(added);
  },
);

it.each([undefined, null, true, 1, {}, [], '', 'compressor', 'high-pass', 'missing-effect'])(
  'rejects unsupported effect and chain request identities: %j',
  (value) => {
    expect(isEffectKind(value)).toBe(false);
    expect(isChainKind(value)).toBe(false);
  },
);
it('keeps effect and chain namespaces distinct', () => {
  expect(isEffectKind('delay')).toBe(true);
  expect(isChainKind('delay')).toBe(false);
  expect(isChainKind('karaoke')).toBe(true);
  expect(isEffectKind('karaoke')).toBe(false);
});

it('validates a translation pack before assigning labels to effects', () => {
  const complete = Object.values(effectTranslations.en).join('|');
  expect(effectLabelsFromWords(complete)).toEqual(effectTranslations.en);
  expect(() => effectLabelsFromWords('Incomplete')).toThrow('Effect translation requires');
  expect(() => effectLabelsFromWords(`${complete}|Unexpected`)).toThrow(
    'Effect translation requires',
  );
});

it('copies defaults and caller parameters so edits cannot mutate another new node or preset', () => {
  const values = { timeMs: 400, feedback: 0.4, mix: 0.6 };
  const config = effectConfig('delay', values);
  config.effect.parameters.mix = 0;
  expect(values.mix).toBe(0.6);
  const generated = effectConfig('delay');
  generated.effect.parameters.timeMs = 20;
  expect(effectDefaults.delay.timeMs).toBe(150);
  expect(effectConfig('delay').effect.parameters.timeMs).toBe(150);
});

it('accepts only complete finite parameters and keeps them in control order', () => {
  expect(effectParameters('pan', null)).toBeNull();
  expect(effectParameters('pan', { position: Number.POSITIVE_INFINITY })).toBeNull();
  expect(
    effectParameters('equalizer', { ...effectDefaults.equalizer, midQ: 'invalid' }),
  ).toBeNull();
  expect(effectParameters('delay', { timeMs: 1, feedback: 0, mix: 1 })).toBeNull();
  const { mode, ...rest } = effectDefaults.delay;
  const parsed = effectParameters('delay', { mode, extra: 1, ...rest });
  expect(parsed).toEqual(effectDefaults.delay);
  expect(Object.keys(parsed ?? {})).toEqual(Object.keys(effectDefaults.delay));
  const restored = runtimeGraphToAudioGraph({
    ...document,
    nodes: [{ id: 'eq', type: 'effect', label: null, config: effectConfig('equalizer') }],
  });
  expect(first(restored).appearance.title).toBe(defaultEffectLabels.equalizer);
  expect(first(restored).appearance.parameterControls).toHaveLength(10);
});

it('labels the EQ middle band like the outer bands, with English fallback labels', () => {
  const controls = effectControls('equalizer', effectDefaults.equalizer, false, {});
  expect(controls.find((c) => c.id === 'midQ')).toMatchObject({
    label: defaultEffectLabels.q,
    step: 0.001,
    group: { id: 'mid-band', label: defaultEffectLabels.midDb },
  });
  expect(controls.find((c) => c.id === 'midFrequencyHz')).toMatchObject({
    label: defaultEffectLabels.frequencyHz,
    value: 1000,
    scale: 'logarithmic',
    group: { id: 'mid-band', label: defaultEffectLabels.midDb },
  });
});

it.each(effectKinds)(
  'clamps %s controls at their declared DSP bounds and rejects invalid commands',
  (kind) => {
    const model = addRoutingNode(empty, { kind: 'effect', effect: kind }, position);
    const node = first(model);
    const control = node.appearance.parameterControls?.find((c) => !c.choices);
    if (!control) return;
    const upper = updateRoutingNodeParameter(model, node.id, control.id, control.maximum + 1000);
    const lower = updateRoutingNodeParameter(upper, node.id, control.id, control.minimum - 1000);
    expect(first(upper).appearance.parameterControls?.find((c) => c.id === control.id)?.value).toBe(
      control.maximum,
    );
    expect(first(lower).appearance.parameterControls?.find((c) => c.id === control.id)?.value).toBe(
      control.minimum,
    );
    expect(updateRoutingNodeParameter(lower, node.id, control.id, control.minimum)).toBe(lower);
    expect(updateRoutingNodeParameter(lower, node.id, control.id, Infinity)).toBe(lower);
    expect(updateRoutingNodeParameter(lower, node.id, 'unknown', 1)).toBe(lower);
    expect(updateRoutingNodeParameter(lower, 'missing-node', control.id, 1)).toBe(lower);
  },
);

it.each(['reverb', 'chorus', 'delay'] as const)(
  'applies every %s preset atomically without changing the preset catalog',
  (kind) => {
    let model = addRoutingNode(empty, { kind: 'effect', effect: kind }, position);
    const id = first(model).id;
    for (const [index, preset] of (effectPresets[kind] ?? []).entries()) {
      const catalogBefore = JSON.stringify(effectPresets);
      const previous = model;
      model = updateRoutingNodeParameter(model, id, 'preset', index + 1);
      expect(model.revision).toBe(previous.revision + 1);
      expect(first(model).runtime?.config).toMatchObject({
        effect: { kind, parameters: preset.parameters },
      });
      expect(first(model).appearance.parameterControls?.find((c) => c.id === 'preset')?.value).toBe(
        index + 1,
      );
      expect(updateRoutingNodeParameter(model, id, 'preset', 0)).toBe(model);
      const saved = audioGraphToRuntimeGraph(model, document);
      expect(saved.nodes[0]?.config).not.toHaveProperty('preset');
      expect(JSON.stringify(effectPresets)).toBe(catalogBefore);
    }
  },
);

it.each(Object.entries(effectTranslations))(
  'retains %s preset, bypass and mode translations across edits and saved restoration',
  (_locale, labels) => {
    const translated = presentation(labels);
    let reverb = addRoutingNode(empty, { kind: 'effect', effect: 'reverb' }, position, translated);
    const id = first(reverb).id;
    reverb = updateRoutingNodeParameter(reverb, id, 'preset', 3);
    reverb = updateRoutingNodeParameter(reverb, id, 'bypass', 1);
    const original = first(reverb).appearance.parameterControls;
    expect(original?.find((c) => c.id === 'preset')?.choices?.[3]?.label).toBe(labels.hall);
    expect(original?.find((c) => c.id === 'bypass')?.choices?.[1]?.label).toBe(labels.bypassed);
    const restored = runtimeGraphToAudioGraph(
      audioGraphToRuntimeGraph(reverb, document),
      translated,
    );
    expect(first(restored).appearance.parameterControls).toEqual(original);
    expect(first(restored).appearance.title).toBe(labels.reverb);
    let utility = addRoutingNode(
      empty,
      { kind: 'effect', effect: 'channel_utility' },
      position,
      translated,
    );
    const utilityId = first(utility).id;
    utility = updateRoutingNodeParameter(utility, utilityId, 'mode', 4);
    const mode = first(utility).appearance.parameterControls?.find((c) => c.id === 'mode');
    expect(mode?.value).toBe(4);
    expect(mode?.choices?.map((c) => c.label)).toEqual(
      (['unchanged', 'mono', 'left', 'right', 'swap', 'invert'] as const).map((key) => labels[key]),
    );
    const edited = updateRoutingNodeParameter(utility, utilityId, 'bypass', 1);
    const active = updateRoutingNodeParameter(edited, utilityId, 'bypass', 0);
    expect(first(active).appearance.parameterControls?.find((c) => c.id === 'mode')).toEqual(mode);
    expect(first(active).runtime?.config).toMatchObject({ bypass: false });
  },
);

it.each([
  null,
  {},
  { effect: null },
  { effect: { kind: 'unavailable', parameters: {} } },
  { effect: { kind: 'equalizer', parameters: { lowDb: -3, midQ: 'invalid' } } },
  { effect: { kind: 'delay', parameters: { timeMs: 1, feedback: 0, mix: 1 } } },
  { ...effectConfig('delay'), channels: undefined },
  (({ bypass: _bypass, ...config }) => config)(effectConfig('delay')),
  { ...effectConfig('delay'), bypass: 'false' },
  { ...effectConfig('delay'), bypass: 0 },
])(
  'preserves unsupported stored effect configuration instead of inventing a replacement: %j',
  (config) => {
    const original: RuntimeGraphDocument = {
      ...document,
      nodes: [{ id: 'unsupported', type: 'effect', label: 'Saved effect', config }],
    };
    const model = runtimeGraphToAudioGraph(original);
    expect(first(model).appearance).toMatchObject({
      category: 'Unsupported',
      title: 'Saved effect',
    });
    expect(first(model).inputPorts).toEqual([]);
    expect(first(model).outputPorts).toEqual([]);
    expect(audioGraphToRuntimeGraph(model, original).nodes[0]?.config).toEqual(config);
  },
);

it('retains stored Off/Mute separately from bypass and restores the chosen node label', () => {
  const original: RuntimeGraphDocument = {
    ...document,
    nodes: [
      {
        id: 'delay',
        type: 'effect',
        label: 'My echo',
        config: { ...effectConfig('delay'), bypass: true },
        control: { enabled: false, muted: true },
      },
    ],
  };
  const model = runtimeGraphToAudioGraph(original);
  expect(first(model).runtime?.control).toEqual({ enabled: false, muted: true });
  expect(first(model).appearance.title).toBe('My echo');
  const edited = updateRoutingNodeParameter(model, 'delay', 'bypass', 0);
  const saved = audioGraphToRuntimeGraph(edited, original);
  expect(saved.nodes[0]?.control).toEqual({ enabled: false, muted: true });
  expect(saved.nodes[0]?.config).toMatchObject({ bypass: false });
  expect(saved.nodes[0]?.label).toBe('My echo');
});

it.each(['reverb', 'ducking'] as const)(
  'falls back to default %s labels for incomplete locale packs while retaining the config',
  (kind) => {
    const translated = presentation({ bypass: 'Localized bypass' });
    const model = addRoutingNode(empty, { kind: 'effect', effect: kind }, position, translated);
    expect(first(model).appearance.title).toBe(defaultEffectLabels[kind]);
    const edited = updateRoutingNodeParameter(model, first(model).id, 'bypass', 1);
    expect(
      first(edited).appearance.parameterControls?.find((c) => c.id === 'bypass'),
    ).toMatchObject({
      label: 'Localized bypass',
      value: 1,
    });
    if (kind === 'ducking') {
      expect(first(model).inputPorts.at(-1)?.label).toContain(defaultEffectLabels.sidechain);
    }
  },
);

it('keeps malformed editable effect contracts unchanged instead of persisting arbitrary parameter keys', () => {
  const model = addRoutingNode(empty, { kind: 'effect', effect: 'delay' }, position);
  const node = first(model);
  for (const config of [
    { effect: null },
    { effect: { kind: 'unknown' } },
    { effect: { kind: 'delay', parameters: { mix: 1 } } },
  ]) {
    const malformed = { ...model, nodes: [{ ...node, runtime: { type: 'effect', config } }] };
    expect(updateRoutingNodeParameter(malformed, node.id, 'mix', 0.5)).toBe(malformed);
  }
});

it('handles missing saved bypass metadata as processing and preserves unknown fields for the engine validator', () => {
  const model = addRoutingNode(empty, { kind: 'effect', effect: 'delay' }, position);
  const node = first(model);
  const config = { ...effectConfig('delay'), extraSavedField: 'retain' };
  delete (config as { bypass?: boolean }).bypass;
  const missing = { ...model, nodes: [{ ...node, runtime: { type: 'effect', config } }] };
  const edited = updateRoutingNodeParameter(missing, node.id, 'feedback', 0.4);
  expect(first(edited).runtime?.config).toMatchObject({
    extraSavedField: 'retain',
    effect: { parameters: { feedback: 0.4 } },
  });
  expect(first(edited).appearance.parameterControls?.find((c) => c.id === 'bypass')?.value).toBe(0);
});

it('regenerates localized controls when optional choice presentation metadata is absent', () => {
  const model = addRoutingNode(empty, { kind: 'effect', effect: 'reverb' }, position);
  const node = first(model);
  const metadata = {
    ...model,
    nodes: [
      {
        ...node,
        appearance: {
          ...node.appearance,
          parameterControls: node.appearance.parameterControls?.map((control) => ({
            ...control,
            choices: undefined,
          })),
        },
      },
    ],
  };
  const edited = updateRoutingNodeParameter(metadata, node.id, 'mix', 0.4);
  expect(
    first(edited).appearance.parameterControls?.find((c) => c.id === 'bypass')?.choices,
  ).toEqual([
    { value: 0, label: defaultEffectLabels.processing },
    { value: 1, label: defaultEffectLabels.bypassed },
  ]);
});

it('refreshes EQ controls from confirmed values, keeping retained labels and defaulting the rest', () => {
  const model = addRoutingNode(
    empty,
    { kind: 'effect', effect: 'equalizer' },
    position,
    presentation(effectTranslations.koKR),
  );
  const node = first(model);
  const partialView = {
    ...model,
    nodes: [
      {
        ...node,
        appearance: {
          ...node.appearance,
          parameterControls: node.appearance.parameterControls
            ?.filter((control) =>
              ['bypass', 'lowDb', 'midDb', 'highDb', 'midFrequencyHz', 'midQ'].includes(control.id),
            )
            .map((control) => ({ ...control, group: undefined })),
        },
      },
    ],
  };
  const edited = updateRoutingNodeParameter(partialView, node.id, 'midDb', 6);
  const controls = first(edited).appearance.parameterControls;
  expect(controls).toHaveLength(10);
  expect(controls?.find((control) => control.id === 'midDb')).toMatchObject({
    label: effectTranslations.koKR.midDb,
    value: 6,
    group: { id: 'mid-band', label: defaultEffectLabels.midDb },
  });
  expect(controls?.find((control) => control.id === 'lowFrequencyHz')).toMatchObject({
    label: defaultEffectLabels.frequencyHz,
    value: 150,
    scale: 'logarithmic',
  });
  expect(audioGraphToRuntimeGraph(edited, document).nodes[0]?.config).toMatchObject({
    effect: { parameters: { midDb: 6, lowFrequencyHz: 150 } },
  });
});

it('does not create a graph revision for a numeric command that rounds to the active parameter', () => {
  const model = addRoutingNode(empty, { kind: 'effect', effect: 'delay' }, position);
  const id = first(model).id;
  expect(updateRoutingNodeParameter(model, id, 'feedback', 0.251)).toBe(model);
  expect(updateRoutingNodeParameter(model, id, 'bypass', 0.01)).toBe(model);
});

it.each([
  'plain',
  'external:',
  'external:-1',
  'external:1.5',
  'external:nope',
  'external:9007199254740992',
])('rejects invalid external parameter identity without persisting it: %s', (id) => {
  const provider: AudioGraphModel = {
    ...empty,
    nodes: [
      {
        id: 'provider',
        position,
        inputPorts: [],
        outputPorts: [],
        appearance: {
          title: 'External effect',
          category: 'Processor',
          icon: 'FX',
          parameterControls: [
            { id, label: 'Wet', minimum: 0, maximum: 1, step: 0.01, suffix: '', value: 0.25 },
          ],
        },
        runtime: {
          type: 'provider',
          config: { typeId: 'vst3:test', parameters: { parameterValues: { '1': 0.25 } } },
        },
      },
    ],
  };
  expect(updateRoutingNodeParameter(provider, 'provider', id, 0.5)).toBe(provider);
});

it('canonicalizes a valid external parameter ID and preserves rounded no-op commands', () => {
  const provider: AudioGraphModel = {
    ...empty,
    nodes: [
      {
        id: 'provider',
        position,
        inputPorts: [],
        outputPorts: [],
        appearance: {
          title: 'External effect',
          category: 'Processor',
          icon: 'FX',
          parameterControls: [
            {
              id: 'external:1',
              label: 'Wet',
              minimum: 0,
              maximum: 1,
              step: 0.01,
              suffix: '',
              value: 0.25,
            },
          ],
        },
        runtime: {
          type: 'provider',
          config: { typeId: 'vst3:test', parameters: { parameterValues: { '1': 0.25 } } },
        },
      },
    ],
  };
  expect(updateRoutingNodeParameter(provider, 'provider', 'external:1', 0.251)).toBe(provider);
  const edited = updateRoutingNodeParameter(provider, 'provider', 'external:1', 0.5);
  expect(first(edited).runtime?.config).toMatchObject({
    parameters: { parameterValues: { '1': 0.5 } },
  });
});

it('discards an invalid preset action without altering a modified effect', () => {
  const model = addRoutingNode(empty, { kind: 'effect', effect: 'reverb' }, position);
  const custom = updateRoutingNodeParameter(model, first(model).id, 'mix', 0.41);
  expect(updateRoutingNodeParameter(custom, first(custom).id, 'preset', 0)).toBe(custom);
  expect(updateRoutingNodeParameter(custom, first(custom).id, 'preset', NaN)).toBe(custom);
});

it('retains an existing graph and its routes when a chain is inserted', () => {
  const existing = addRoutingNode(empty, { kind: 'effect', effect: 'delay' }, position);
  const before = JSON.stringify(existing);
  const extended = addRoutingNode(
    existing,
    { kind: 'effect_chain', preset: 'radio' },
    { x: 400, y: 20 },
  );
  expect(extended.nodes[0]).toBe(existing.nodes[0]);
  expect(JSON.stringify(existing)).toBe(before);
  expect(extended.nodes.slice(1).map((node) => node.runtime?.type)).toEqual([
    'effect',
    'effect',
    'compressor',
  ]);
  expect(
    extended.edges.every(
      (edge) =>
        edge.sourceNodeId !== first(existing).id && edge.targetNodeId !== first(existing).id,
    ),
  ).toBe(true);
  expect(extended.revision).toBe(existing.revision + 1);
});

it('applies frequency and gain together as one validated equalizer edit', () => {
  const model = addRoutingNode(empty, { kind: 'effect', effect: 'equalizer' }, position);
  const node = first(model);
  const next = updateRoutingNodeParameters(model, node.id, [
    { id: 'lowFrequencyHz', value: 355.3 },
    { id: 'lowDb', value: 30 },
    { id: 'lowQ', value: 1.2344 },
    { id: 'highFrequencyHz', value: Number.NaN },
    { id: 'unknown', value: 4 },
  ]);
  expect(next.revision).toBe(model.revision + 1);
  expect(first(next).runtime?.config).toMatchObject({
    effect: {
      parameters: {
        lowFrequencyHz: 355,
        lowDb: 24,
        lowQ: 1.234,
        highFrequencyHz: 6000,
      },
    },
  });
  expect(first(model).runtime?.config).toMatchObject({
    effect: { parameters: { lowFrequencyHz: 150, lowDb: 0 } },
  });
  expect(updateRoutingNodeParameters(next, node.id, [])).toBe(next);
  expect(
    updateRoutingNodeParameters(next, node.id, [
      { id: 'lowDb', value: 24 },
      { id: 'midQ', value: Infinity },
    ]),
  ).toBe(next);
  expect(updateRoutingNodeParameters(next, 'missing', [{ id: 'lowDb', value: 0 }])).toBe(next);
});

it('keeps duplicate inventory entries without icons on the shared fallback', () => {
  const selector = {
    match: 'bundle_id' as const,
    bundle_id: 'org.example.player',
    include_descendants: false,
  };
  const application = {
    bundleId: selector.bundle_id,
    displayName: 'Player',
    iconDataUrl: null,
    processId: 10,
    selector,
  };
  const model = runtimeGraphToAudioGraph({
    ...document,
    nodes: [
      {
        id: 'app',
        type: 'application_capture',
        label: 'Player',
        config: { selector, outputChannels: 2, gainDb: 0 },
      },
    ],
  });
  const stale = {
    ...model,
    nodes: model.nodes.map((node) => ({
      ...node,
      appearance: { ...node.appearance, iconUrl: 'old-icon' },
    })),
  };
  const result = presentRoutingApplicationIcons(stale, [
    application,
    { ...application, processId: 11 },
  ]);
  expect(first(result).appearance.iconUrl).toBeUndefined();
});

it('serializes shared virtual Off/Mute alongside an added processor', () => {
  const original = {
    ...document,
    nodes: [
      {
        id: 'virtual',
        type: 'virtual_endpoint',
        label: null,
        control: { enabled: false, muted: true },
        config: { ...virtualSpec, reference: duplexReference, gainDb: 0 },
      },
    ],
  };
  const model = runtimeGraphToAudioGraph(original);
  const mixed = addRoutingNode(model, { kind: 'effect', effect: 'delay' }, position);
  const saved = audioGraphToRuntimeGraph(mixed, original);
  expect(saved.nodes.find((node) => node.id === 'virtual')?.control).toEqual({
    enabled: false,
    muted: true,
  });
});

it('treats a generic processor value rounding to its existing setting as a no-op', () => {
  const model = addRoutingNode(empty, { kind: 'compressor' }, position);
  expect(updateRoutingNodeParameter(model, first(model).id, 'thresholdDb', -18.1)).toBe(model);
});

it('drops stale unknown preset choices while retaining the localized Custom label', () => {
  const model = addRoutingNode(
    empty,
    { kind: 'effect', effect: 'reverb' },
    position,
    presentation(effectTranslations.koKR),
  );
  const node = first(model);
  const stale = {
    ...model,
    nodes: [
      {
        ...node,
        appearance: {
          ...node.appearance,
          parameterControls: node.appearance.parameterControls?.map((control) =>
            control.id === 'preset'
              ? {
                  ...control,
                  choices: [
                    ...(control.choices ?? []),
                    { value: 99, label: 'Unavailable old preset' },
                  ],
                }
              : control,
          ),
        },
      },
    ],
  };
  const next = updateRoutingNodeParameter(stale, node.id, 'mix', 0.5);
  const choices = first(next).appearance.parameterControls?.find(
    (control) => control.id === 'preset',
  )?.choices;
  expect(choices?.find((choice) => choice.value === 0)?.label).toBe(effectTranslations.koKR.custom);
  expect(choices?.some((choice) => choice.value === 99)).toBe(false);
});

it('undo restores both axes of a graph gesture together', () => {
  const model = addRoutingNode(empty, { kind: 'effect', effect: 'equalizer' }, position);
  const next = updateRoutingNodeParameters(model, first(model).id, [
    { id: 'midFrequencyHz', value: 2500 },
    { id: 'midDb', value: 6 },
  ]);
  const { result } = renderHook(() => useUndoHistory<AudioGraphModel, 'runtime'>());
  act(() => {
    result.current.record(model, next, 'runtime');
  });
  expect(result.current.undo()?.value).toBe(model);
  expect(result.current.undo()).toBeNull();
  expect(result.current.redo()?.value).toBe(next);
});

it.each([
  ['lowFrequencyHz', 20, 20000],
  ['midFrequencyHz', 20, 20000],
  ['highFrequencyHz', 20, 20000],
  ['lowQ', 0.1, 10],
  ['midQ', 0.1, 10],
  ['highQ', 0.1, 10],
  ['lowDb', -24, 24],
  ['midDb', -24, 24],
  ['highDb', -24, 24],
] as const)('validates and persists the entire allowed %s range', (id, minimum, maximum) => {
  const model = addRoutingNode(empty, { kind: 'effect', effect: 'equalizer' }, position);
  const nodeId = first(model).id;
  const upper = updateRoutingNodeParameter(model, nodeId, id, maximum + 100);
  const lower = updateRoutingNodeParameter(upper, nodeId, id, minimum - 100);
  expect(first(upper).runtime?.config).toMatchObject({ effect: { parameters: { [id]: maximum } } });
  expect(first(lower).runtime?.config).toMatchObject({ effect: { parameters: { [id]: minimum } } });
  const saved = audioGraphToRuntimeGraph(lower, document);
  const restored = runtimeGraphToAudioGraph(saved);
  expect(
    first(restored).appearance.parameterControls?.find((control) => control.id === id)?.value,
  ).toBe(minimum);
  expect(updateRoutingNodeParameter(lower, nodeId, id, Infinity)).toBe(lower);
});

it.each(Object.entries(effectTranslations))(
  'localizes every EQ band and frequency/Q control in %s',
  (_locale, labels) => {
    const model = addRoutingNode(
      empty,
      { kind: 'effect', effect: 'equalizer' },
      position,
      presentation(labels),
    );
    expect(first(model).appearance.equalizer?.bands.map((band) => band.label)).toEqual([
      labels.lowDb,
      labels.midDb,
      labels.highDb,
    ]);
    const controls = first(model).appearance.parameterControls;
    for (const id of ['lowFrequencyHz', 'midFrequencyHz', 'highFrequencyHz'])
      expect(controls?.find((control) => control.id === id)?.label).toBe(labels.frequencyHz);
    for (const id of ['lowQ', 'midQ', 'highQ'])
      expect(controls?.find((control) => control.id === id)?.label).toBe(labels.q);
  },
);

it('uses complete English EQ band labels for a partial translated presentation', () => {
  const model = addRoutingNode(
    empty,
    { kind: 'effect', effect: 'equalizer' },
    position,
    presentation({ equalizer: 'Custom EQ' }),
  );
  expect(first(model).appearance.equalizer?.bands.map((band) => band.label)).toEqual([
    'Low',
    'Mid',
    'High',
  ]);
  expect(
    first(model).appearance.parameterControls?.find((control) => control.id === 'lowFrequencyHz')
      ?.label,
  ).toBe('Frequency');
});

it('leaves the full 320px EQ face and its controls unobstructed when inserting the Tunnel chain', () => {
  const graph = addRoutingNode(empty, { kind: 'effect_chain', preset: 'tunnel' }, position);
  expect(graph.nodes.map((node) => node.position)).toEqual([
    { x: 30, y: 40 },
    { x: 382, y: 40 },
    { x: 642, y: 40 },
  ]);
  expect(graph.nodes[0]?.appearance.equalizer?.bands).toHaveLength(3);
  expect(graph.nodes[1]?.position.x).toBeGreaterThan((graph.nodes[0]?.position.x ?? 0) + 320);
  expect(graph.edges).toHaveLength(4);
  expect(graph.revision).toBe(empty.revision + 1);
});

it('retains the existing 260px placement interval for standard-width chain nodes', () => {
  const graph = addRoutingNode(empty, { kind: 'effect_chain', preset: 'radio' }, position);
  expect(graph.nodes.map((node) => node.position.x)).toEqual([30, 290, 550]);
  expect(graph.nodes.every((node) => node.position.y === position.y)).toBe(true);
  expect(graph.edges).toHaveLength(4);
});
