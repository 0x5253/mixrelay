import { getConfig, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useEffect, useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { advance, eventually, find, findIn, finishFakeTimerTest } from './fakeTimerQueries';

function Delayed({ ms }: { ms: number }) {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setShown(true), ms);
    return () => clearTimeout(timer);
  }, [ms]);
  return shown ? <button type="button">Ready</button> : null;
}

afterEach(() => vi.useRealTimers());

const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;

it('awaits pointer and keyboard input without advancing background polling', async () => {
  vi.useFakeTimers();
  const click = vi.fn();
  const poll = vi.fn();
  const interval = setInterval(poll, 250);
  render(
    <button type="button" onClick={click}>
      Map control
    </button>,
  );
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
  const input = user.click(screen.getByRole('button', { name: 'Map control' }));
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    // Bound the regression without leaving RTL's async wrapper pending on failure.
    const completed = await Promise.race([
      input.then(() => true),
      new Promise<boolean>((resolve) => {
        timeout = realSetTimeout(() => resolve(false), 1000);
      }),
    ]);
    expect(completed).toBe(true);
    await user.keyboard('{Enter}');
    expect(click).toHaveBeenCalledTimes(2);
    expect(poll).not.toHaveBeenCalled();
  } finally {
    realClearTimeout(timeout);
    clearInterval(interval);
    await advance();
    await input;
  }
});

it('restores the async environment after a rejected operation so the next render works', async () => {
  vi.useFakeTimers();
  await expect(
    getConfig().asyncWrapper(() => Promise.reject(new Error('input failed'))),
  ).rejects.toThrow('input failed');
  const click = vi.fn();
  render(
    <button type="button" onClick={click}>
      Retry
    </button>,
  );
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
  await user.click(screen.getByRole('button', { name: 'Retry' }));
  expect(click).toHaveBeenCalledOnce();
});

it('steps the fake clock until a timer-driven element appears, without wall-clock waiting', async () => {
  vi.useFakeTimers();
  render(<Delayed ms={1000} />);
  // No real-clock budget: only stepping the fake clock can reveal the element.
  expect(await find('button', { name: 'Ready' }, { realTimeout: 0 })).toBeInTheDocument();
});

it('fails once the fake-time limit passes', async () => {
  vi.useFakeTimers();
  render(<Delayed ms={5000} />);
  await expect(
    eventually(() => screen.getByRole('button', { name: 'Ready' }), {
      timeout: 500,
      realTimeout: 50,
    }),
  ).rejects.toThrow();
});

/** Shows its button from a MessageChannel message, which fake timers do not control. */
function RealAsync() {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const channel = new MessageChannel();
    channel.port1.onmessage = () => setShown(true);
    channel.port2.postMessage(null);
    return () => channel.port1.close();
  }, []);
  return shown ? <button type="button">Real</button> : null;
}

it('keeps checking on the real clock once fake time is used up', async () => {
  vi.useFakeTimers();
  render(<RealAsync />);
  expect(await find('button', { name: 'Real' }, { timeout: 0 })).toBeInTheDocument();
});

it('defers to Testing Library under real timers', async () => {
  render(<Delayed ms={10} />);
  await advance();
  expect(await findIn(screen, 'button', { name: 'Ready' })).toBeInTheDocument();
  await eventually(() => expect(screen.getByRole('button', { name: 'Ready' })).toBeVisible());
  await eventually(() => expect(screen.getByRole('button', { name: 'Ready' })).toBeVisible(), {
    timeout: 1000,
  });
});

it('teardown unmounts and restores real timers and mocks even when draining rejects', async () => {
  vi.useFakeTimers();
  const spy = vi.spyOn(console, 'info').mockImplementation(() => {});
  const restored = vi.fn();
  render(<button type="button">Mounted</button>);
  await expect(
    finishFakeTimerTest(() => Promise.reject(new Error('drain failed')), restored),
  ).rejects.toThrow('drain failed');
  expect(screen.queryByRole('button', { name: 'Mounted' })).not.toBeInTheDocument();
  expect(vi.isFakeTimers()).toBe(false);
  expect(vi.isMockFunction(console.info)).toBe(false);
  expect(spy).not.toHaveBeenCalled();
  expect(restored).toHaveBeenCalledOnce();
});
