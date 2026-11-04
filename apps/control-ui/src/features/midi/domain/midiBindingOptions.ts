import { type MidiBinding, midiAction, midiMode } from '../../../core/domain/midi';

/** One select of a mapping's options; `apply` returns the binding with the chosen value. */
export interface MidiOptionField {
  name: 'recorderAction' | 'mode' | 'min' | 'max' | 'step';
  value: string;
  choices: readonly string[];
  apply(value: string): MidiBinding;
}

/** Absolute volume controls pick up within a decibel range; relative ones move by a step. */
export type MidiBindingOptions =
  | { kind: 'recorder'; action: MidiOptionField }
  | { kind: 'absolute'; mode: MidiOptionField; range: readonly [MidiOptionField, MidiOptionField] }
  | { kind: 'relative'; mode: MidiOptionField; step: MidiOptionField };

const recorderActions = ['record', 'pause', 'stop'];
const volumeModes = ['absolute', 'twos_complement', 'offset64'];
const decibelRange = Array.from({ length: 145 }, (_, index) => String(-60 + index * 0.5));
const decibelSteps = ['0.5', '1', '2', '3'];

/** The options a mapping offers; power and mute have none. */
export function midiBindingOptions(binding: MidiBinding): MidiBindingOptions | null {
  if (recorderActions.includes(binding.action))
    return {
      kind: 'recorder',
      action: {
        name: 'recorderAction',
        value: binding.action,
        choices: recorderActions,
        apply: (value) => ({ ...binding, action: midiAction(value) }),
      },
    };
  if (binding.action !== 'volume') return null;
  const mode: MidiOptionField = {
    name: 'mode',
    value: binding.mode,
    choices: volumeModes,
    apply: (value) => ({ ...binding, mode: midiMode(value) }),
  };
  if (binding.mode !== 'absolute')
    return {
      kind: 'relative',
      mode,
      step: {
        name: 'step',
        value: String(binding.stepDb),
        choices: decibelSteps,
        apply: (value) => ({ ...binding, stepDb: Number(value) }),
      },
    };
  return {
    kind: 'absolute',
    mode,
    range: [
      {
        name: 'min',
        value: String(binding.minDb),
        choices: decibelRange,
        apply: (value) => ({ ...binding, minDb: Number(value) }),
      },
      {
        name: 'max',
        value: String(binding.maxDb),
        choices: decibelRange,
        apply: (value) => ({ ...binding, maxDb: Number(value) }),
      },
    ],
  };
}
