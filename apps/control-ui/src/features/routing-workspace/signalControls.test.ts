import { expect, it } from 'vitest';
import { signalControlVisibility, usesRecoveryControls } from './signalControls';

const both = { power: true, mute: true };
const normal = { enabled: true, muted: false };

it.each(['effect', 'compressor', 'limiter'])(
  'a built-in %s uses its own Bypass and shows Off and Mute only for recovery',
  (type) => {
    expect(usesRecoveryControls(type, false)).toBe(true);
    // MIDI mapping exposes every engine control for assignment.
    expect(usesRecoveryControls(type, true)).toBe(false);
  },
);

it.each(['application_capture', 'audio_device_input', 'provider', undefined])(
  'a %s node shows its reported controls normally',
  (type) => {
    expect(usesRecoveryControls(type, false)).toBe(false);
  },
);

it('a recovery-only node with nothing to recover shows no signal controls', () => {
  expect(signalControlVisibility(both, normal, true)).toBeNull();
});

it('recovery shows only the control that undoes the authored state', () => {
  expect(signalControlVisibility(both, { enabled: false, muted: false }, true)).toEqual({
    power: true,
    mute: false,
  });
  expect(signalControlVisibility(both, { enabled: true, muted: true }, true)).toEqual({
    power: false,
    mute: true,
  });
  expect(signalControlVisibility(both, { enabled: false, muted: true }, true)).toEqual(both);
});

it('a control appears only where the engine reports that capability', () => {
  expect(signalControlVisibility(both, normal, false)).toEqual(both);
  expect(signalControlVisibility({ power: true, mute: false }, normal, false)).toEqual({
    power: true,
    mute: false,
  });
  expect(signalControlVisibility(undefined, normal, false)).toEqual({ power: false, mute: false });
  expect(signalControlVisibility(undefined, { enabled: false, muted: true }, true)).toEqual({
    power: false,
    mute: false,
  });
});
