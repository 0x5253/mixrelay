export const midiActions = ['power', 'mute', 'volume', 'record', 'pause', 'stop'] as const;
export type MidiAction = (typeof midiActions)[number];
export const midiModes = ['absolute', 'twos_complement', 'offset64'] as const;
export type MidiMode = (typeof midiModes)[number];

export interface MidiDevice {
  id: string;
  name: string;
}
export interface MidiControl {
  device: MidiDevice;
  channel: number;
  kind: 'note' | 'cc';
  number: number;
}
export interface MidiTarget {
  graphId: string;
  nodeId: string;
}
export interface MidiAssignment {
  target: MidiTarget;
  action: MidiAction;
  label: string;
  graphName: string;
  nodeName: string;
}
export interface MidiBinding {
  control: MidiControl;
  target: MidiTarget;
  action: MidiAction;
  mode: MidiMode;
  stepDb: number;
  graphName: string;
  nodeName: string;
  minDb: number;
  maxDb: number;
}
export type MidiLearn = { state: 'idle' | 'waiting' } | { state: 'captured'; control: MidiControl };
export interface MidiStatus {
  mapMode: boolean;
  targetsAvailable: boolean;
  bindings: MidiBinding[];
  devices: MidiDevice[];
  unavailable: string | null;
  learn: MidiLearn;
  targets: { target: MidiTarget; label: string }[];
}

export function midiAction(value: unknown): MidiAction {
  const action = midiActions.find((candidate) => candidate === value);
  if (action === undefined) throw new Error('Invalid MIDI action');
  return action;
}

export function midiMode(value: unknown): MidiMode {
  const mode = midiModes.find((candidate) => candidate === value);
  if (mode === undefined) throw new Error('Invalid MIDI mode');
  return mode;
}

export const sameControl = (a: MidiControl, b: MidiControl) =>
  a.device.id === b.device.id &&
  a.channel === b.channel &&
  a.kind === b.kind &&
  a.number === b.number;
export const sameTarget = (a: MidiTarget, b: MidiTarget) =>
  a.graphId === b.graphId && a.nodeId === b.nodeId;
export const controlKey = (control: MidiControl) =>
  JSON.stringify([control.device.id, control.channel, control.kind, control.number]);
export const bindingKey = (binding: MidiBinding) =>
  JSON.stringify([
    controlKey(binding.control),
    binding.target.graphId,
    binding.target.nodeId,
    binding.action,
  ]);
export function bindingTarget(status: MidiStatus, binding: MidiBinding) {
  return status.targets.find((item) => sameTarget(item.target, binding.target));
}
export function disconnected(status: MidiStatus, control: MidiControl) {
  return !status.devices.some((device) => device.id === control.device.id);
}
export function bindingLabel(binding: MidiBinding) {
  return `${binding.graphName} / ${binding.nodeName}`;
}

export function shortControl(control: MidiControl, noteName: string) {
  if (control.kind === 'cc') return `CC ${control.number}`;
  const notes = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];
  return `${noteName} ${notes[control.number % 12]}${Math.floor(control.number / 12) - 2}`;
}
