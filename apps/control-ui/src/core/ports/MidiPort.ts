import type { MidiStatus } from '../domain/midi';

export type MidiCommand =
  | 'midi_status'
  | 'midi_map'
  | 'midi_learn'
  | 'midi_cancel'
  | 'midi_save'
  | 'midi_remove';

export interface MidiPort {
  command(name: MidiCommand, args?: Record<string, unknown>): Promise<MidiStatus>;
  subscribeMapToggle?(listener: () => void): () => void;
}
