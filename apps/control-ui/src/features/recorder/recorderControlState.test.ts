import { expect, it } from 'vitest';
import type { RecorderStatus } from '../../core/domain/control/recorder';
import {
  type RecorderControlInputs,
  recorderControlState,
  recordingRateOptions,
} from './domain/recorderControlState';

const stopped: RecorderStatus = {
  state: 'stopped',
  path: null,
  sampleRate: null,
  encoding: null,
  frames: 0,
  error: null,
};
const ready: RecorderControlInputs = {
  status: stopped,
  settings: { folder: '/Desktop', sampleRate: 48000, encoding: 'pcm24' },
  settingsError: null,
  busy: false,
  enabled: true,
  poweredOn: true,
  playbackPaused: false,
};
const take = (state: RecorderStatus['state']): RecorderStatus => ({
  ...stopped,
  state,
  path: '/Desktop/take.wav',
  sampleRate: 96000,
  encoding: 'float32',
});
const buttons = (inputs: RecorderControlInputs) => {
  const state = recorderControlState(inputs);
  return [state.record.enabled, state.pause.enabled, state.stop.enabled];
};

it('starts a take only when the graph routes audio in and settings are saved', () => {
  expect(buttons(ready)).toEqual([true, false, false]);
  expect(recorderControlState(ready).fieldsLocked).toBe(false);
  expect(buttons({ ...ready, enabled: false })).toEqual([false, false, false]);
  expect(buttons({ ...ready, settings: null })).toEqual([false, false, false]);
  expect(buttons({ ...ready, settingsError: 'Preferences unavailable' })).toEqual([
    false,
    false,
    false,
  ]);
});

it('a running take can pause and stop, locks its format and shows its own values', () => {
  const running = recorderControlState({ ...ready, status: take('recording') });
  expect([running.record.enabled, running.pause.enabled, running.stop.enabled]).toEqual([
    false,
    true,
    true,
  ]);
  expect(running.record.active).toBe(true);
  expect(running.fieldsLocked).toBe(true);
  expect([running.sampleRate, running.encoding]).toEqual([96000, 'float32']);
});

it('a paused take resumes, even after its graph stops routing or its settings fail to load', () => {
  const paused = recorderControlState({ ...ready, status: take('paused') });
  expect(paused.record).toEqual({ enabled: true, resumes: true, active: false });
  expect(buttons({ ...ready, status: take('paused'), enabled: false })).toEqual([
    true,
    false,
    true,
  ]);
  expect(
    buttons({ ...ready, status: take('paused'), settings: null, settingsError: 'unreadable' }),
  ).toEqual([true, false, true]);
});

it('paused app playback leaves only Stop, so a take can still be finalized', () => {
  expect(buttons({ ...ready, status: take('paused'), playbackPaused: true })).toEqual([
    false,
    false,
    true,
  ]);
  expect(buttons({ ...ready, status: take('recording'), playbackPaused: true })).toEqual([
    false,
    false,
    true,
  ]);
});

it('an engine that is off, a request in flight or unknown status allow no command', () => {
  for (const inputs of [
    { ...ready, status: take('paused'), poweredOn: false },
    { ...ready, busy: true },
    { ...ready, status: null },
  ])
    expect(buttons(inputs)).toEqual([false, false, false]);
  expect(recorderControlState({ ...ready, busy: true }).fieldsLocked).toBe(true);
});

it('shows the saved settings while stopped and falls back to the source rate and 16-bit', () => {
  expect(recorderControlState(ready)).toMatchObject({ sampleRate: 48000, encoding: 'pcm24' });
  expect(recorderControlState({ ...ready, settings: null })).toMatchObject({
    sampleRate: null,
    encoding: 'pcm16',
  });
});

it('offers the product rates plus a nonstandard rate a take actually uses', () => {
  expect(recordingRateOptions(null).map(({ value }) => value)).toEqual([
    '44100',
    '48000',
    '88200',
    '96000',
    '176400',
    '192000',
  ]);
  expect(recordingRateOptions(32000).at(-1)).toEqual({ value: '32000', label: '32 kHz' });
  expect(recordingRateOptions(48000)).toHaveLength(6);
});
