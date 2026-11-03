import { type EffectTranslationKey, effectTranslations } from '../../core/i18n/effects';
import type { AudioNodeParameterControl } from '../../core/ui/AudioNodeParameterGrid';

export const effectKinds = [
  'equalizer',
  'high_pass',
  'low_pass',
  'gate',
  'expander',
  'channel_utility',
  'pan',
  'balance',
  'delay',
  'ducking',
  'reverb',
  'chorus',
] as const;
export type EffectKind = (typeof effectKinds)[number];
export const chainKinds = ['karaoke', 'tunnel', 'fishbowl', 'radio', 'doubleVocal'] as const;
export type ChainKind = (typeof chainKinds)[number];
export type EffectLabels = Record<string, string>;
export const defaultEffectLabels = effectTranslations.en;

const standardQ = Number(Math.SQRT1_2.toFixed(3));
const channelModeDescriptions = [
  'unchangedDescription',
  'monoDescription',
  'leftDescription',
  'rightDescription',
  'swapDescription',
  'invertDescription',
] as const;
export const effectDefaults: Record<EffectKind, Record<string, number>> = {
  equalizer: {
    lowDb: 0,
    lowFrequencyHz: 150,
    lowQ: standardQ,
    midDb: 0,
    midFrequencyHz: 1000,
    midQ: standardQ,
    highDb: 0,
    highFrequencyHz: 6000,
    highQ: standardQ,
  },
  high_pass: { frequencyHz: 80, q: standardQ },
  low_pass: { frequencyHz: 8000, q: standardQ },
  gate: { thresholdDb: -45, ratio: 4, attackMs: 5, releaseMs: 150, rangeDb: 80 },
  expander: { thresholdDb: -45, ratio: 2, attackMs: 5, releaseMs: 150, rangeDb: 24 },
  channel_utility: { mode: 0 },
  pan: { position: 0 },
  balance: { position: 0 },
  delay: { timeMs: 150, feedback: 0.25, mix: 0.2, mode: 0 },
  ducking: { thresholdDb: -30, reductionDb: 12, attackMs: 15, releaseMs: 300 },
  reverb: { decaySeconds: 1.2, damping: 0.5, mix: 0.2, roomSize: 1, preDelayMs: 0, algorithm: 0 },
  chorus: { rateHz: 0.6, depthMs: 3, mix: 0.3 },
};

interface Preset {
  label: string;
  parameters: Record<string, number>;
}
export const effectPresets: Partial<Record<EffectKind, readonly Preset[]>> = {
  reverb: [
    {
      label: 'smallRoom',
      parameters: {
        decaySeconds: 0.4,
        damping: 0.6,
        mix: 0.15,
        roomSize: 0.6,
        preDelayMs: 4,
        algorithm: 1,
      },
    },
    {
      label: 'studio',
      parameters: {
        decaySeconds: 0.8,
        damping: 0.5,
        mix: 0.12,
        roomSize: 0.8,
        preDelayMs: 12,
        algorithm: 1,
      },
    },
    {
      label: 'hall',
      parameters: {
        decaySeconds: 3,
        damping: 0.35,
        mix: 0.3,
        roomSize: 1.5,
        preDelayMs: 28,
        algorithm: 1,
      },
    },
    {
      label: 'karaoke',
      parameters: {
        decaySeconds: 1.4,
        damping: 0.45,
        mix: 0.2,
        roomSize: 1.1,
        preDelayMs: 22,
        algorithm: 1,
      },
    },
    {
      label: 'tunnel',
      parameters: {
        decaySeconds: 4.5,
        damping: 0.75,
        mix: 0.4,
        roomSize: 2,
        preDelayMs: 65,
        algorithm: 1,
      },
    },
    {
      label: 'fishbowl',
      parameters: {
        decaySeconds: 0.7,
        damping: 0.9,
        mix: 0.25,
        roomSize: 0.5,
        preDelayMs: 0,
        algorithm: 1,
      },
    },
  ],
  chorus: [
    { label: 'subtle', parameters: { rateHz: 0.3, depthMs: 1, mix: 0.15 } },
    { label: 'wide', parameters: { rateHz: 0.8, depthMs: 5, mix: 0.4 } },
  ],
  delay: [
    { label: 'shortEcho', parameters: { timeMs: 90, feedback: 0.15, mix: 0.2, mode: 0 } },
    { label: 'slapback', parameters: { timeMs: 75, feedback: 0, mix: 0.25, mode: 0 } },
    { label: 'vocalEcho', parameters: { timeMs: 220, feedback: 0.3, mix: 0.2, mode: 0 } },
    { label: 'karaoke', parameters: { timeMs: 140, feedback: 0.25, mix: 0.18, mode: 0 } },
    { label: 'longEcho', parameters: { timeMs: 480, feedback: 0.5, mix: 0.3, mode: 0 } },
    { label: 'pingPong', parameters: { timeMs: 300, feedback: 0.55, mix: 0.35, mode: 1 } },
  ],
};
const ranges = {
  lowDb: [-24, 24, 0.1, 'dB'],
  midDb: [-24, 24, 0.1, 'dB'],
  highDb: [-24, 24, 0.1, 'dB'],
  frequencyHz: [20, 20000, 1, 'Hz'],
  lowFrequencyHz: [20, 20000, 1, 'Hz'],
  midFrequencyHz: [20, 20000, 1, 'Hz'],
  highFrequencyHz: [20, 20000, 1, 'Hz'],
  lowQ: [0.1, 10, 0.001, ''],
  midQ: [0.1, 10, 0.001, ''],
  highQ: [0.1, 10, 0.001, ''],
  q: [0.1, 10, 0.001, ''],
  thresholdDb: [-80, 0, 0.1, 'dB'],
  ratio: [1, 20, 0.1, ':1'],
  attackMs: [0.1, 200, 0.1, 'ms'],
  releaseMs: [5, 2000, 1, 'ms'],
  rangeDb: [0, 80, 1, 'dB'],
  mode: [0, 5, 1, ''],
  position: [-1, 1, 0.01, ''],
  timeMs: [0, 2000, 0.1, 'ms'],
  feedback: [0, 0.9, 0.01, ''],
  mix: [0, 1, 0.01, ''],
  reductionDb: [0, 60, 0.1, 'dB'],
  decaySeconds: [0.1, 6, 0.1, 's'],
  damping: [0, 0.95, 0.01, ''],
  roomSize: [0.5, 2, 0.01, ''],
  preDelayMs: [0, 200, 0.1, 'ms'],
  algorithm: [0, 1, 1, ''],
  rateHz: [0.05, 5, 0.01, 'Hz'],
  depthMs: [0, 10, 0.1, 'ms'],
} satisfies Record<string, readonly [number, number, number, string]>;
export function isEffectKind(value: unknown): value is EffectKind {
  return typeof value === 'string' && effectKinds.some((kind) => kind === value);
}
export function isChainKind(value: unknown): value is ChainKind {
  return typeof value === 'string' && chainKinds.some((kind) => kind === value);
}
/** Every parameter of one effect kind, in its control order. */
export type EffectParameters = Record<string, number>;

/** Returns exactly the parameters of `kind` when each one is a finite number; otherwise null. */
export function effectParameters(kind: EffectKind, value: unknown): EffectParameters | null {
  if (typeof value !== 'object' || value === null) return null;
  const parameters: EffectParameters = {};
  for (const id of Object.keys(effectDefaults[kind])) {
    const parameter: unknown = Reflect.get(value, id);
    if (typeof parameter !== 'number' || !Number.isFinite(parameter)) return null;
    parameters[id] = parameter;
  }
  return parameters;
}

export function effectConfig(kind: EffectKind, parameters = effectDefaults[kind]) {
  return {
    version: 1,
    channels: kind === 'pan' ? 1 : 2,
    bypass: false,
    effect: { kind, parameters: { ...parameters } },
  };
}

export function effectControls(
  kind: EffectKind,
  parameters: EffectParameters,
  bypass: boolean,
  labels: EffectLabels,
): AudioNodeParameterControl[] {
  const text = (key: string) => labels[key] ?? defaultEffectLabels[key as EffectTranslationKey];
  const presets = effectPresets[kind] ?? [];
  const selected =
    presets.findIndex((p) =>
      Object.entries(p.parameters).every(([key, value]) => parameters[key] === value),
    ) + 1;
  const controls: AudioNodeParameterControl[] = [
    {
      id: 'bypass',
      label: text('bypass'),
      minimum: 0,
      maximum: 1,
      step: 1,
      suffix: '',
      value: Number(bypass),
      choices: [
        { value: 0, label: text('processing') },
        { value: 1, label: text('bypassed') },
      ],
    },
  ];
  if (presets.length)
    controls.push({
      id: 'preset',
      label: text('preset'),
      minimum: 0,
      maximum: presets.length,
      step: 1,
      suffix: '',
      value: selected,
      choices: [
        { value: 0, label: text('custom') },
        ...presets.map((p, index) => ({ value: index + 1, label: text(p.label) })),
      ],
    });
  for (const [id, value] of Object.entries(parameters)) {
    if (kind === 'gate' && id === 'ratio') continue;
    const [minimum, maximum, step, suffix] = ranges[id as keyof typeof ranges];
    controls.push({
      id,
      label: text(effectParameterLabelKey(kind, id)),
      minimum,
      maximum: kind === 'delay' && id === 'mode' ? 1 : maximum,
      step,
      suffix,
      value,
      precise: true,
      presentation: 'knob',
      section: parameterSection(kind, id),
      ...controlPresentation(kind, id, text),
    });
  }
  return controls;
}

/** Refresh confirmed values without reverse-mapping presentation back into translation keys. */
export function refreshEffectControls(
  kind: EffectKind,
  parameters: EffectParameters,
  bypass: boolean,
  previous: readonly AudioNodeParameterControl[],
): AudioNodeParameterControl[] {
  return effectControls(kind, parameters, bypass, defaultEffectLabels).map((control) => {
    const retained = previous.find((candidate) => candidate.id === control.id);
    if (!retained) return control;
    return {
      ...control,
      label: retained.label,
      group: control.group
        ? { ...control.group, label: retained.group?.label ?? control.group.label }
        : undefined,
      choices: control.choices?.map((choice) => {
        const previousChoice = retained.choices?.find(
          (candidate) => candidate.value === choice.value,
        );
        return {
          ...choice,
          label: previousChoice?.label ?? choice.label,
          description: previousChoice?.description ?? choice.description,
        };
      }),
    };
  });
}

function parameterSection(kind: EffectKind, id: string) {
  return kind === 'reverb' && ['algorithm', 'roomSize', 'preDelayMs'].includes(id)
    ? ('options' as const)
    : ('main' as const);
}
function effectParameterLabelKey(kind: EffectKind, id: string) {
  if (id === 'position') return kind === 'pan' ? 'panPosition' : 'balancePosition';
  if (kind === 'delay' && id === 'mode') return 'delayMode';
  if (id.endsWith('FrequencyHz')) return 'frequencyHz';
  if (id.endsWith('Q')) return 'q';
  return id;
}

function effectParameterChoiceKeys(kind: EffectKind, id: string) {
  if (id === 'algorithm') return ['classicReverb', 'denseReverb'];
  if (id !== 'mode') return undefined;
  return kind === 'delay'
    ? ['stereoEcho', 'pingPong']
    : ['unchanged', 'mono', 'left', 'right', 'swap', 'invert'];
}
function effectParameterDescriptionKey(kind: EffectKind, id: string, value: number) {
  return kind === 'channel_utility' && id === 'mode' ? channelModeDescriptions[value] : undefined;
}
function parameterChoices(kind: EffectKind, id: string, text: (key: string) => string) {
  return effectParameterChoiceKeys(kind, id)?.map((label, value) => {
    const description = effectParameterDescriptionKey(kind, id, value);
    return { value, label: text(label), description: description ? text(description) : undefined };
  });
}
function controlPresentation(kind: EffectKind, id: string, text: (key: string) => string) {
  return {
    endpoints: id === 'position' ? { minimum: 'L', maximum: 'R' } : undefined,
    group:
      kind === 'equalizer'
        ? {
            id: id.startsWith('low')
              ? 'low-band'
              : id.startsWith('high')
                ? 'high-band'
                : 'mid-band',
            label: text(
              id.startsWith('low') ? 'lowDb' : id.startsWith('high') ? 'highDb' : 'midDb',
            ),
          }
        : undefined,
    scale:
      id === 'frequencyHz' || id.endsWith('FrequencyHz')
        ? ('logarithmic' as const)
        : ('linear' as const),
    choices: parameterChoices(kind, id, text),
  };
}

export function chainEffects(
  kind: ChainKind,
): { kind: EffectKind; parameters: Record<string, number> }[] {
  const make = (effect: EffectKind, patch: Record<string, number>) => ({
    kind: effect,
    parameters: { ...effectDefaults[effect], ...patch },
  });
  switch (kind) {
    case 'karaoke':
      return [
        make('delay', { timeMs: 140, feedback: 0.25, mix: 0.15 }),
        make('reverb', {
          decaySeconds: 1.4,
          damping: 0.45,
          mix: 0.2,
          roomSize: 1.1,
          preDelayMs: 22,
          algorithm: 1,
        }),
      ];
    case 'tunnel':
      return [
        make('equalizer', { highDb: -8, lowDb: -2 }),
        make('delay', { timeMs: 360, feedback: 0.55, mix: 0.25 }),
        make('reverb', {
          decaySeconds: 4.5,
          damping: 0.75,
          mix: 0.4,
          roomSize: 2,
          preDelayMs: 65,
          algorithm: 1,
        }),
      ];
    case 'fishbowl':
      return [
        make('high_pass', { frequencyHz: 250 }),
        make('low_pass', { frequencyHz: 2200 }),
        make('chorus', { rateHz: 0.4, depthMs: 4, mix: 0.45 }),
        make('reverb', {
          decaySeconds: 0.7,
          damping: 0.9,
          mix: 0.15,
          roomSize: 0.5,
          preDelayMs: 0,
          algorithm: 1,
        }),
      ];
    case 'radio':
      return [make('high_pass', { frequencyHz: 300 }), make('low_pass', { frequencyHz: 3000 })];
    case 'doubleVocal':
      return [
        make('chorus', { rateHz: 0.6, depthMs: 2, mix: 0.35 }),
        make('delay', { timeMs: 18, feedback: 0, mix: 0.25 }),
      ];
  }
}
