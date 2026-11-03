import { expect, it, vi } from 'vitest';
import type { MidiPort } from '../../../core/ports/MidiPort';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import { MidiSession } from './MidiSession';

it('sends no command and publishes no state after its owner is disposed', async () => {
  const time = fakeScheduler();
  const port: MidiPort = { command: vi.fn() };
  const publish = vi.fn();
  const owner = new MidiSession(port, time, publish);
  owner.start(false);
  publish.mockClear();
  owner.dispose();
  expect(await owner.command('midi_cancel')).toBe(false);
  expect(port.command).not.toHaveBeenCalled();
  expect(publish).not.toHaveBeenCalled();
  expect(time.pending()).toBe(0);
});
