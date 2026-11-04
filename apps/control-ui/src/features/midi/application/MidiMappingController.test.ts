import { describe, expect, it, vi } from 'vitest';
import { midiAssignment, midiBinding } from '../midiFixtures';
import { type MidiKeyEvent, MidiMappingController, midiShortcut } from './MidiMappingController';

function controller(confirmed = true) {
  const send = vi.fn(async () => confirmed);
  const effects = { select: vi.fn(), release: vi.fn() };
  return { send, effects, mapping: new MidiMappingController(send, effects) };
}

describe('MidiMappingController', () => {
  it('enters map mode without a selection and leaves it through the same toggle', async () => {
    const { send, effects, mapping } = controller();
    await mapping.toggle(false);
    expect(send).toHaveBeenLastCalledWith('midi_map', { enabled: true });
    expect(effects.select).toHaveBeenLastCalledWith(null);
    expect(effects.release).not.toHaveBeenCalled();
    await mapping.toggle(true);
    expect(send).toHaveBeenLastCalledWith('midi_cancel');
    expect(effects.release).toHaveBeenCalledOnce();
  });

  it('learns only the target, action and remembered names and selects the assignment', async () => {
    const { send, effects, mapping } = controller();
    await mapping.assign(midiAssignment);
    expect(send).toHaveBeenCalledWith('midi_learn', {
      assignment: {
        target: midiAssignment.target,
        action: 'volume',
        graphName: 'Programme',
        nodeName: 'Microphone',
      },
    });
    expect(effects.select).toHaveBeenCalledWith(midiAssignment);
  });

  it('removes one complete mapping identity, then clears the selection and releases focus', async () => {
    const { send, effects, mapping } = controller();
    await mapping.remove(midiBinding);
    expect(send).toHaveBeenCalledWith('midi_remove', { binding: midiBinding });
    expect(effects.select).toHaveBeenCalledWith(null);
    expect(effects.release).toHaveBeenCalledOnce();
  });

  it('saves edited options under the old identity and keeps the edited mapping selected', async () => {
    const { send, effects, mapping } = controller();
    const edited = { ...midiBinding, action: 'stop' as const };
    expect(await mapping.save(midiBinding, edited)).toBe(true);
    expect(send).toHaveBeenCalledWith('midi_save', { identity: midiBinding, binding: edited });
    expect(effects.select).toHaveBeenCalledWith({ ...edited, label: 'Microphone' });
  });

  it('keeps the current mapping state when the session does not confirm a command', async () => {
    const { effects, mapping } = controller(false);
    await mapping.toggle(false);
    await mapping.toggle(true);
    await mapping.assign(midiAssignment);
    await mapping.remove(midiBinding);
    expect(await mapping.save(midiBinding, midiBinding)).toBe(false);
    expect(effects.select).not.toHaveBeenCalled();
    expect(effects.release).not.toHaveBeenCalled();
  });
});

describe('midiShortcut', () => {
  const key = (event: Partial<MidiKeyEvent>): MidiKeyEvent => ({
    key: 'm',
    repeat: false,
    isComposing: false,
    altKey: false,
    shiftKey: false,
    defaultPrevented: false,
    ...event,
  });

  it('toggles map mode with the platform modifier and M only', () => {
    expect(midiShortcut(key({}), true, false)).toBe('toggle');
    expect(midiShortcut(key({ key: 'M' }), true, true)).toBe('toggle');
    expect(midiShortcut(key({}), false, false)).toBeNull();
    expect(midiShortcut(key({ altKey: true }), true, false)).toBeNull();
    expect(midiShortcut(key({ shiftKey: true }), true, false)).toBeNull();
  });

  it('ignores repeated and composing key presses', () => {
    expect(midiShortcut(key({ repeat: true }), true, false)).toBeNull();
    expect(midiShortcut(key({ isComposing: true }), true, false)).toBeNull();
    expect(midiShortcut(key({ key: 'Escape', repeat: true }), false, true)).toBeNull();
  });

  it('ends map mode with an unhandled Escape while it is active', () => {
    expect(midiShortcut(key({ key: 'Escape' }), false, true)).toBe('end');
    expect(midiShortcut(key({ key: 'Escape' }), false, false)).toBeNull();
    expect(midiShortcut(key({ key: 'Escape', defaultPrevented: true }), false, true)).toBeNull();
    expect(midiShortcut(key({ key: 'Delete' }), true, true)).toBeNull();
  });
});
