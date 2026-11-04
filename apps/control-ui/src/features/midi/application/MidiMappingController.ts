import type { MidiAssignment, MidiBinding } from '../../../core/domain/midi';
import type { MidiCommand } from '../../../core/ports/MidiPort';

/** Sends one MIDI command; true only when the current session confirmed it. */
type MidiCommandSender = (name: MidiCommand, args?: Record<string, unknown>) => Promise<boolean>;

export interface MidiMappingEffects {
  /** The assignment now waiting for a hardware control, or none. */
  select(assignment: MidiAssignment | null): void;
  /** Map mode or a mapping ended on request; focus returns to the map-mode toggle. */
  release(): void;
}

/** Map-mode commands. Selection changes only after the native session confirms a command,
 * so a failed or superseded command leaves the current mapping state as it was. */
export class MidiMappingController {
  constructor(
    private readonly send: MidiCommandSender,
    private readonly effects: MidiMappingEffects,
  ) {}

  async toggle(active: boolean) {
    if (active) return this.end();
    if (await this.send('midi_map', { enabled: true })) this.effects.select(null);
  }

  async end() {
    if (await this.send('midi_cancel')) this.release();
  }

  async assign(assignment: MidiAssignment) {
    const { target, action, graphName, nodeName } = assignment;
    if (await this.send('midi_learn', { assignment: { target, action, graphName, nodeName } }))
      this.effects.select(assignment);
  }

  async remove(binding: MidiBinding) {
    if (await this.send('midi_remove', { binding })) this.release();
  }

  /** Saves edited options of `identity`; the edited mapping stays selected. */
  async save(identity: MidiBinding, binding: MidiBinding) {
    const saved = await this.send('midi_save', { identity, binding });
    if (saved) this.effects.select({ ...binding, label: binding.nodeName });
    return saved;
  }

  private release() {
    this.effects.select(null);
    this.effects.release();
  }
}

export interface MidiKeyEvent {
  key: string;
  repeat: boolean;
  isComposing: boolean;
  altKey: boolean;
  shiftKey: boolean;
  defaultPrevented: boolean;
}

/** The map-mode command a window key press requests: the platform modifier with M toggles
 * map mode and Escape leaves it unless another control already handled the key. */
export function midiShortcut(
  event: MidiKeyEvent,
  platformModifier: boolean,
  active: boolean,
): 'toggle' | 'end' | null {
  if (event.repeat || event.isComposing) return null;
  if (event.key.toLowerCase() === 'm' && platformModifier && !event.altKey && !event.shiftKey)
    return 'toggle';
  if (event.key === 'Escape' && active && !event.defaultPrevented) return 'end';
  return null;
}
