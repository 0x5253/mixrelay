import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { fakeNativeShell } from '../../test-support/fakeNativeShell';
import { useNativeGraphListCommand } from './useNativeGraphListCommand';

it('runs the latest toggle from the View menu and mirrors visibility to its label', async () => {
  const native = fakeNativeShell();
  const first = vi.fn();
  const latest = vi.fn();
  const view = renderHook(({ visible, toggle }) => useNativeGraphListCommand(visible, toggle), {
    initialProps: { visible: true, toggle: first },
    wrapper: native.Provider,
  });
  await waitFor(() => expect(native.listening('graphList')).toBe(1));
  expect(native.shell.setGraphListVisible).toHaveBeenLastCalledWith(true);
  view.rerender({ visible: false, toggle: latest });
  expect(native.shell.setGraphListVisible).toHaveBeenLastCalledWith(false);
  act(() => native.fire('graphList'));
  expect(latest).toHaveBeenCalledOnce();
  expect(first).not.toHaveBeenCalled();
  expect(native.shell.onGraphListToggle).toHaveBeenCalledOnce();
  view.unmount();
  expect(native.removals.graphList).toHaveBeenCalledOnce();
});

it('releases a listener that registers after unmount and stays inert outside the app', async () => {
  const native = fakeNativeShell();
  const release = native.hold();
  const view = renderHook(() => useNativeGraphListCommand(true, vi.fn()), {
    wrapper: native.Provider,
  });
  view.unmount();
  await act(async () => release());
  expect(native.removals.graphList).toHaveBeenCalledOnce();
  expect(native.listening('graphList')).toBe(0);
  // Without a shell there is nothing to bind or label.
  renderHook(() => useNativeGraphListCommand(true, vi.fn()));
});

it('logs native listener and label failures without throwing', async () => {
  const native = fakeNativeShell();
  native.shell.onGraphListToggle.mockRejectedValue(new Error('no event bus'));
  native.shell.setGraphListVisible.mockRejectedValue(new Error('no command'));
  const error = vi.spyOn(console, 'error').mockImplementation(() => {});
  try {
    renderHook(() => useNativeGraphListCommand(true, vi.fn()), { wrapper: native.Provider });
    await waitFor(() => expect(error).toHaveBeenCalledTimes(2));
    expect(error).toHaveBeenCalledWith('Graph list menu listener failed', expect.any(Error));
    expect(error).toHaveBeenCalledWith('Graph list menu label failed', expect.any(Error));
  } finally {
    error.mockRestore();
  }
});
