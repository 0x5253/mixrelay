import { act, renderHook } from '@testing-library/react';
import { StrictMode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type { MidiStatus } from '../../core/domain/midi';
import type { MidiPort } from '../../core/ports/MidiPort';
import { advance } from '../../test-support/fakeTimerQueries';
import { midiFixture } from './midiFixtures';
import { useMidiSession } from './useMidiSession';

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});
function deferred() {
  let resolve: (value: MidiStatus) => void = () => {};
  let reject: (reason: Error) => void = () => {};
  const promise = new Promise<MidiStatus>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
it('Strict Mode cleanup and port replacement discard late reads and stop polling', async () => {
  vi.useFakeTimers();
  const first = deferred();
  const second = deferred();
  const a: MidiPort = {
    command: vi.fn().mockReturnValueOnce(first.promise).mockReturnValue(second.promise),
  };
  const b: MidiPort = { command: vi.fn().mockResolvedValue(midiFixture) };
  const hook = renderHook(({ port, opened }) => useMidiSession(port, opened), {
    initialProps: { port: a, opened: true },
    wrapper: StrictMode,
  });
  hook.rerender({ port: b, opened: true });
  await advance();
  expect(hook.result.current.status).toEqual(midiFixture);
  await act(async () => {
    first.resolve({ ...midiFixture, unavailable: 'stale' });
    second.reject(new Error('stale'));
  });
  expect(hook.result.current.status?.unavailable).toBeNull();
  hook.rerender({ port: b, opened: false });
  await advance(1000);
  expect(b.command).toHaveBeenCalledTimes(1);
});
it('failed and successful mutations publish only to their current owner', async () => {
  vi.useFakeTimers();
  const pending = deferred();
  const port: MidiPort = {
    command: vi
      .fn()
      .mockResolvedValueOnce(midiFixture)
      .mockRejectedValueOnce(new Error('failed'))
      .mockResolvedValueOnce(midiFixture)
      .mockReturnValue(pending.promise),
  };
  const hook = renderHook(() => useMidiSession(port, true));
  await advance();
  await act(async () => expect(await hook.result.current.command('midi_remove', {})).toBe(false));
  expect(hook.result.current.error).toBe(true);
  await act(async () => expect(await hook.result.current.command('midi_cancel')).toBe(true));
  expect(hook.result.current.error).toBe(false);
  let result: Promise<boolean>;
  act(() => {
    result = hook.result.current.command('midi_save');
  });
  expect(hook.result.current.busy).toBe(true);
  hook.unmount();
  await act(async () => {
    pending.resolve(midiFixture);
    expect(await result).toBe(false);
  });
});

it('a late status read cannot undo an acknowledged map mode transition', async () => {
  vi.useFakeTimers();
  const pending = deferred();
  const port: MidiPort = {
    command: vi
      .fn()
      .mockReturnValueOnce(pending.promise)
      .mockResolvedValue({ ...midiFixture, mapMode: true }),
  };
  const hook = renderHook(() => useMidiSession(port, true));
  await act(async () => {
    await hook.result.current.command('midi_map', { enabled: true });
  });
  await act(async () => {
    pending.resolve(midiFixture);
  });
  expect(hook.result.current.status?.mapMode).toBe(true);
});
it('an obsolete command failure does not mark a replacement port unavailable', async () => {
  vi.useFakeTimers();
  const pending = deferred();
  const first: MidiPort = {
    command: vi.fn().mockResolvedValueOnce(midiFixture).mockReturnValue(pending.promise),
  };
  const second: MidiPort = { command: vi.fn().mockResolvedValue(midiFixture) };
  const hook = renderHook(({ port }) => useMidiSession(port, true), {
    initialProps: { port: first },
  });
  await advance();
  let result: Promise<boolean>;
  act(() => {
    result = hook.result.current.command('midi_cancel');
  });
  hook.rerender({ port: second });
  await advance();
  await act(async () => {
    pending.reject(new Error('old owner'));
    expect(await result).toBe(false);
  });
  expect(hook.result.current.error).toBe(false);
});

it.each(['success', 'failure'] as const)(
  'superseded mutation %s cannot restore map mode, caller selection, error or busy state',
  async (outcome) => {
    vi.useFakeTimers();
    const learn = deferred();
    const cancel = deferred();
    const port: MidiPort = {
      command: vi
        .fn()
        .mockResolvedValueOnce(midiFixture)
        .mockReturnValueOnce(learn.promise)
        .mockReturnValueOnce(cancel.promise),
    };
    const hook = renderHook(() => useMidiSession(port, true));
    await advance();
    let selected = false;
    let learning: Promise<void>;
    let cancelling: Promise<boolean>;
    act(() => {
      learning = hook.result.current.command('midi_learn').then((accepted) => {
        if (accepted) selected = true;
      });
    });
    act(() => {
      cancelling = hook.result.current.command('midi_cancel');
    });
    await act(async () => {
      if (outcome === 'success') learn.resolve({ ...midiFixture, mapMode: true });
      else learn.reject(new Error('superseded'));
      await learning;
    });
    expect(selected).toBe(false);
    expect(hook.result.current.status?.mapMode).toBe(false);
    expect(hook.result.current.error).toBe(false);
    expect(hook.result.current.busy).toBe(true);
    await act(async () => {
      cancel.resolve(midiFixture);
      expect(await cancelling).toBe(true);
    });
    expect(hook.result.current.busy).toBe(false);
  },
);

it('late learn success after cancel completes cannot re-enter map mode', async () => {
  vi.useFakeTimers();
  const learn = deferred();
  const port: MidiPort = {
    command: vi
      .fn()
      .mockResolvedValueOnce(midiFixture)
      .mockReturnValueOnce(learn.promise)
      .mockResolvedValue(midiFixture),
  };
  const hook = renderHook(() => useMidiSession(port, true));
  await advance();
  let learning: Promise<boolean>;
  act(() => {
    learning = hook.result.current.command('midi_learn');
  });
  await act(async () => {
    await hook.result.current.command('midi_cancel');
  });
  await act(async () => {
    learn.resolve({ ...midiFixture, mapMode: true });
    expect(await learning).toBe(false);
  });
  expect(hook.result.current.status?.mapMode).toBe(false);
});

it('a status read started during a mutation cannot replace that mutations newer reply', async () => {
  vi.useFakeTimers();
  const mutation = deferred();
  const read = deferred();
  const port: MidiPort = {
    command: vi
      .fn()
      .mockResolvedValueOnce(midiFixture)
      .mockReturnValueOnce(mutation.promise)
      .mockReturnValueOnce(read.promise),
  };
  const hook = renderHook(() => useMidiSession(port, true));
  await advance();
  let result: Promise<boolean>;
  act(() => {
    result = hook.result.current.command('midi_map', { enabled: true });
  });
  await advance(250);
  await act(async () => {
    mutation.resolve({ ...midiFixture, mapMode: true });
    expect(await result).toBe(true);
  });
  await act(async () => {
    read.resolve(midiFixture);
  });
  expect(hook.result.current.status?.mapMode).toBe(true);
});
