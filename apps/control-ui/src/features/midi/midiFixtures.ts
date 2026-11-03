import type { MidiAssignment, MidiBinding, MidiStatus } from '../../core/domain/midi';
export const midiAssignment: MidiAssignment = {
  target: { graphId: 'main', nodeId: 'gain' },
  action: 'volume',
  label: 'Programme / Microphone',
  graphName: 'Programme',
  nodeName: 'Microphone',
};
export const midiBinding: MidiBinding = {
  control: {
    device: { id: 'coremidi:42', name: 'Studio Controller' },
    channel: 0,
    kind: 'cc',
    number: 7,
  },
  target: midiAssignment.target,
  action: 'volume',
  mode: 'absolute',
  stepDb: 1,
  graphName: 'Programme',
  nodeName: 'Microphone',
  minDb: -60,
  maxDb: 12,
};
export const midiFixture: MidiStatus = {
  mapMode: false,
  targetsAvailable: true,
  bindings: [midiBinding],
  devices: [midiBinding.control.device],
  unavailable: null,
  learn: { state: 'idle' },
  targets: [{ target: midiAssignment.target, label: midiAssignment.label }],
};

/** A controllable IPC boundary for mapping stories and UI behaviour tests. */
export function midiPortFixture(initial: MidiStatus = midiFixture) {
  let status = structuredClone(initial);
  return {
    setStatus(next: MidiStatus) {
      status = structuredClone(next);
    },
    async command(name: import('../../core/ports/MidiPort').MidiCommand) {
      if (name === 'midi_map') status = { ...status, mapMode: true };
      if (name === 'midi_cancel') status = { ...status, mapMode: false, learn: { state: 'idle' } };
      if (name === 'midi_learn') status = { ...status, learn: { state: 'waiting' } };
      return structuredClone(status);
    },
  };
}
