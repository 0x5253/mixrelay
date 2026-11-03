import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import { MixRelayProvider } from '../theme/MixRelayProvider';
import { formatPlatformShortcut } from './platformShortcut';
import { ShortcutTooltip } from './ShortcutTooltip';

afterEach(async () => {
  cleanup();
  if (vi.isFakeTimers()) await act(async () => vi.advanceTimersByTimeAsync(2000));
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it.each([
  ['MacIntel', '⌘N'],
  ['Linux x86_64', 'Ctrl+N'],
  ['Linux aarch64', 'Ctrl+N'],
])('renders the new graph tooltip for %s after the hover delay', async (platform, label) => {
  vi.useFakeTimers();
  vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
  render(
    <MixRelayProvider>
      <ShortcutTooltip shortcut="Mod+N">
        <button type="button">New graph</button>
      </ShortcutTooltip>
    </MixRelayProvider>,
  );
  fireEvent.pointerMove(document.body);
  fireEvent.pointerOver(screen.getByRole('button', { name: 'New graph' }));
  if (platform === 'MacIntel') expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  await act(async () => vi.advanceTimersByTimeAsync(350));
  // React Aria completes hover state on the following animation frame.
  await act(async () => vi.advanceTimersByTimeAsync(200));
  expect(screen.getByRole('tooltip')).toHaveTextContent(label);
  fireEvent.pointerOut(screen.getByRole('button', { name: 'New graph' }));
  await act(async () => vi.advanceTimersByTimeAsync(2000));
});

it('does not rewrite explicit non-Mod keys or unknown shortcuts', () => {
  expect(formatPlatformShortcut('Ctrl+Shift+Z', 'MacIntel')).toBe('Ctrl+Shift+Z');
  expect(formatPlatformShortcut('Space', 'Linux x86_64')).toBe('Space');
  expect(formatPlatformShortcut('Mod+Z', '')).toBe('Ctrl+Z');
});
