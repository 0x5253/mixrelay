import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { version } from '../../../package.json';
import { fakeNativeShell } from '../../test-support/fakeNativeShell';
import { useAppInformation } from './useAppInformation';

it('reads the installed version and opens the latest About callback from the native menu', async () => {
  const native = fakeNativeShell('1.2.3');
  const first = vi.fn();
  const latest = vi.fn();
  const view = renderHook(({ open }) => useAppInformation(open, true), {
    initialProps: { open: first },
    wrapper: native.Provider,
  });
  await waitFor(() => expect(view.result.current).toBe('1.2.3'));
  await waitFor(() => expect(native.listening('about')).toBe(1));
  view.rerender({ open: latest });
  act(() => native.fire('about'));
  expect(latest).toHaveBeenCalledOnce();
  expect(first).not.toHaveBeenCalled();
  view.unmount();
  expect(native.removals.about).toHaveBeenCalledOnce();
});

it('releases a native listener that finishes registering after unmount', async () => {
  const native = fakeNativeShell();
  const release = native.hold();
  const view = renderHook(() => useAppInformation(vi.fn(), true), { wrapper: native.Provider });
  view.unmount();
  await act(async () => release());
  expect(native.removals.about).toHaveBeenCalledOnce();
  expect(native.listening('about')).toBe(0);
});

it('uses the build version without a native shell or while disabled', () => {
  expect(renderHook(() => useAppInformation(vi.fn(), true)).result.current).toBe(version);
  const native = fakeNativeShell();
  renderHook(() => useAppInformation(vi.fn(), false), { wrapper: native.Provider });
  expect(native.shell.version).not.toHaveBeenCalled();
  expect(native.shell.onAbout).not.toHaveBeenCalled();
});

it('keeps product information usable when native version and listener requests fail', async () => {
  const native = fakeNativeShell();
  const versionError = new Error('Native version unavailable');
  const listenerError = new Error('Native menu unavailable');
  native.shell.version.mockRejectedValue(versionError);
  native.shell.onAbout.mockRejectedValue(listenerError);
  const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
  try {
    const view = renderHook(() => useAppInformation(vi.fn(), true), { wrapper: native.Provider });
    await waitFor(() => expect(errors).toHaveBeenCalledTimes(2));
    expect(view.result.current).toBe(version);
    expect(errors).toHaveBeenCalledWith('Could not read app version', versionError);
    expect(errors).toHaveBeenCalledWith('About menu listener failed', listenerError);
  } finally {
    errors.mockRestore();
  }
});

it('ignores a version that arrives after unmount', async () => {
  const native = fakeNativeShell();
  const reply = Promise.withResolvers<string>();
  native.shell.version.mockReturnValue(reply.promise);
  const view = renderHook(() => useAppInformation(vi.fn(), true), { wrapper: native.Provider });
  view.unmount();
  await act(async () => reply.resolve('9.9.9'));
  expect(view.result.current).toBe(version);
});
