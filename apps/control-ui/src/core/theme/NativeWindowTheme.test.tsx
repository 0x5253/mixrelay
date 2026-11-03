import { render, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { fakeNativeShell } from '../../test-support/fakeNativeShell';
import { MixRelayProvider } from './MixRelayProvider';
import { NativeWindowTheme } from './NativeWindowTheme';

it('keeps the native window on the chosen appearance, including system', () => {
  const native = fakeNativeShell();
  const view = render(
    <MixRelayProvider theme="dark">
      <NativeWindowTheme />
    </MixRelayProvider>,
    { wrapper: native.Provider },
  );
  expect(native.shell.setWindowTheme).toHaveBeenLastCalledWith('dark');
  view.rerender(
    <MixRelayProvider theme="system">
      <NativeWindowTheme />
    </MixRelayProvider>,
  );
  expect(native.shell.setWindowTheme).toHaveBeenLastCalledWith('system');
});

it('does nothing without a shell and reports native failures without throwing', async () => {
  render(
    <MixRelayProvider theme="light">
      <NativeWindowTheme />
    </MixRelayProvider>,
  );
  const native = fakeNativeShell();
  native.shell.setWindowTheme.mockRejectedValue(new Error('denied'));
  const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
  render(
    <MixRelayProvider theme="dark">
      <NativeWindowTheme />
    </MixRelayProvider>,
    { wrapper: native.Provider },
  );
  await waitFor(() =>
    expect(error).toHaveBeenCalledWith(
      'Could not update the native window appearance',
      expect.any(Error),
    ),
  );
  error.mockRestore();
});
