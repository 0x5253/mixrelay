import { afterEach, expect, it, vi } from 'vitest';
import {
  bindingTarget,
  controlKey,
  disconnected,
  midiAction,
  midiMode,
  sameControl,
  sameTarget,
} from '../../domain/midi';
import { InvalidResponseError } from '../decode';
import { midiStatus, TauriMidiPort } from './TauriMidiPort';

const control = {
  device: { id: 'uid:42', name: 'Controller' },
  channel: 15,
  number: 127,
  kind: 'cc',
};
const binding = {
  control,
  target: { graphId: 'g', nodeId: 'n' },
  action: 'volume',
  mode: 'absolute',
  stepDb: 1,
  graphName: 'Programme',
  nodeName: 'Microphone',
  minDb: -60,
  maxDb: 12,
};
const raw = {
  mapMode: false,
  bindings: [binding],
  devices: [control.device],
  unavailable: null,
  learn: { state: 'captured', control },
  snapshot: {
    graphs: [
      {
        id: 'g',
        name: 'Programme',
        nodes: [
          { nodeId: 'n', label: 'Microphone' },
          { nodeId: 'other', label: null },
        ],
      },
    ],
  },
};
afterEach(() => {
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
});
it('validates the native response and preserves saved identities independently of display names', async () => {
  const invoke = vi.fn().mockResolvedValue(raw);
  const native = new TauriMidiPort(invoke);
  const status = await native.command('midi_status');
  expect(invoke).toHaveBeenCalledWith('midi_status', undefined);
  expect(status.targets).toEqual([
    { target: binding.target, label: 'Programme / Microphone' },
    { target: { graphId: 'g', nodeId: 'other' }, label: 'Programme / ' },
  ]);
  const saved = status.bindings[0];
  if (!saved) throw new Error('missing binding');
  expect(bindingTarget(status, saved)?.label).toBe('Programme / Microphone');
  expect(disconnected(status, saved.control)).toBe(false);
  expect(disconnected({ ...status, devices: [] }, saved.control)).toBe(true);
  expect(bindingTarget({ ...status, targets: [] }, saved)).toBeUndefined();
  expect(
    sameControl(saved.control, {
      ...saved.control,
      device: { ...saved.control.device, name: 'Renamed' },
    }),
  ).toBe(true);
  for (const changed of [
    { ...saved.control, channel: 0 },
    { ...saved.control, number: 0 },
    { ...saved.control, kind: 'note' as const },
    { ...saved.control, device: { ...saved.control.device, id: 'other' } },
  ])
    expect(sameControl(saved.control, changed)).toBe(false);
  expect(sameTarget(saved.target, { graphId: 'g', nodeId: 'other' })).toBe(false);
  expect(sameTarget(saved.target, { graphId: 'other', nodeId: 'n' })).toBe(false);
  expect(controlKey(saved.control)).toBe('["uid:42",15,"cc",127]');
});
it('accepts every documented option and idle/waiting/timeout states', () => {
  for (const action of ['power', 'mute', 'volume', 'record', 'pause', 'stop'])
    expect(midiAction(action)).toBe(action);
  for (const mode of ['absolute', 'twos_complement', 'offset64']) expect(midiMode(mode)).toBe(mode);
  for (const state of ['idle', 'waiting']) {
    expect(
      midiStatus({ ...raw, learn: { state }, snapshot: null, unavailable: 'No ALSA access' }),
    ).toMatchObject({ learn: { state }, targets: [], unavailable: 'No ALSA access' });
  }
  expect(
    midiStatus({
      ...raw,
      bindings: [
        {
          ...binding,
          control: { ...control, kind: 'note', channel: 0, number: 0 },
          action: 'mute',
          mode: 'offset64',
          stepDb: 0.5,
        },
      ],
    }).bindings[0]?.control.kind,
  ).toBe('note');
});
it('rejects malformed native data instead of presenting invented actionable state', () => {
  const invalid = [
    null,
    { ...raw, mapMode: null },
    { ...raw, bindings: [{ ...binding, minDb: -61 }] },
    { ...raw, bindings: [{ ...binding, maxDb: NaN }] },
    [],
    { ...raw, bindings: {} },
    { ...raw, devices: [{ id: 1, name: 'x' }] },
    { ...raw, learn: { state: 'unknown' } },
    ...['yes', -1, 16, 0.5].map((channel) => ({
      ...raw,
      bindings: [{ ...binding, control: { ...control, channel } }],
    })),
    { ...raw, bindings: [{ ...binding, control: { ...control, number: 128 } }] },
    { ...raw, bindings: [{ ...binding, control: { ...control, kind: 'clock' } }] },
    { ...raw, bindings: [{ ...binding, stepDb: '1' }] },
    { ...raw, bindings: [{ ...binding, stepDb: 4 }] },
    { ...raw, snapshot: { graphs: [{ id: null, nodes: [] }] } },
  ];
  for (const value of invalid) expect(() => midiStatus(value)).toThrow(InvalidResponseError);
  expect(() =>
    midiStatus({ ...raw, bindings: [{ ...binding, control: { ...control, number: 128 } }] }),
  ).toThrow('invalid response at midi.bindings[0].control.number');
  expect(() => midiAction('delete')).toThrow('Invalid MIDI action');
  expect(() => midiMode('guess')).toThrow('Invalid MIDI mode');
});
it('the desktop adapter invokes through Tauri by default', async () => {
  const invoke = vi.fn().mockResolvedValue(raw);
  Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: { invoke } });
  await new TauriMidiPort().command('midi_cancel', {});
  expect(invoke).toHaveBeenCalledWith('midi_cancel', {}, undefined);
});

it('shows musical note labels and distinguishes every target/action sharing a hardware control', async () => {
  const { bindingKey, shortControl, bindingLabel } = await import('../../domain/midi');
  const status = midiStatus(raw);
  const saved = status.bindings[0];
  if (!saved) throw new Error('mapping required');
  expect(shortControl({ ...saved.control, kind: 'note', number: 60 }, 'Note')).toBe('Note C3');
  expect(shortControl({ ...saved.control, kind: 'note', number: 61 }, 'Note')).toBe('Note C♯3');
  expect(shortControl(saved.control, 'Note')).toBe('CC 127');
  expect(bindingKey(saved)).not.toBe(bindingKey({ ...saved, action: 'mute' }));
  expect(bindingKey(saved)).not.toBe(
    bindingKey({ ...saved, target: { ...saved.target, nodeId: 'other' } }),
  );
  expect(bindingLabel(saved)).toBe('Programme / Microphone');
});
