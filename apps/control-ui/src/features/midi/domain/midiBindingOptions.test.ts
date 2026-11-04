import { expect, it } from 'vitest';
import { midiBinding } from '../midiFixtures';
import { midiBindingOptions } from './midiBindingOptions';

it('offers no options for power and mute mappings', () => {
  expect(midiBindingOptions({ ...midiBinding, action: 'power' })).toBeNull();
  expect(midiBindingOptions({ ...midiBinding, action: 'mute' })).toBeNull();
});

it('lets an absolute volume mapping change its mode and pickup range in half-decibel steps', () => {
  const options = midiBindingOptions(midiBinding);
  if (options?.kind !== 'absolute') throw new Error('Expected absolute volume options');
  const [minimum, maximum] = options.range;
  expect(options.mode.value).toBe('absolute');
  expect(options.mode.choices).toEqual(['absolute', 'twos_complement', 'offset64']);
  expect(options.mode.apply('offset64')).toEqual({ ...midiBinding, mode: 'offset64' });
  expect(() => options.mode.apply('unknown')).toThrow('Invalid MIDI mode');
  expect([minimum.name, minimum.value, maximum.name, maximum.value]).toEqual([
    'min',
    '-60',
    'max',
    '12',
  ]);
  expect(minimum.choices).toHaveLength(145);
  expect([minimum.choices[0], minimum.choices[1], minimum.choices.at(-1)]).toEqual([
    '-60',
    '-59.5',
    '12',
  ]);
  expect(minimum.apply('12')).toEqual({ ...midiBinding, minDb: 12 });
  expect(maximum.apply('-60')).toEqual({ ...midiBinding, maxDb: -60 });
});

it('lets a relative volume mapping change its step instead of a range', () => {
  const relative = { ...midiBinding, mode: 'twos_complement' as const };
  const options = midiBindingOptions(relative);
  if (options?.kind !== 'relative') throw new Error('Expected relative volume options');
  expect(options.mode.value).toBe('twos_complement');
  expect(options.step).toMatchObject({ name: 'step', value: '1', choices: ['0.5', '1', '2', '3'] });
  expect(options.step.apply('3')).toEqual({ ...relative, stepDb: 3 });
});

it('lets a recorder mapping switch between recorder actions', () => {
  const record = { ...midiBinding, action: 'record' as const };
  const options = midiBindingOptions(record);
  if (options?.kind !== 'recorder') throw new Error('Expected recorder options');
  expect(options.action).toMatchObject({
    name: 'recorderAction',
    value: 'record',
    choices: ['record', 'pause', 'stop'],
  });
  expect(options.action.apply('stop')).toEqual({ ...record, action: 'stop' });
  expect(midiBindingOptions({ ...midiBinding, action: 'pause' })?.kind).toBe('recorder');
});
