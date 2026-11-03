import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { userEvent } from 'storybook/test';
import { afterEach, expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { motion } from '../theme/tokens';
import { IconActionButton } from './IconActionButton';

afterEach(async () => {
  cleanup();
  if (vi.isFakeTimers()) await act(async () => vi.advanceTimersByTimeAsync(2000));
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('shows the same named tooltip on native pointer hover and removes it after leaving', async () => {
  vi.useFakeTimers();
  render(
    <MixRelayProvider>
      <IconActionButton label="Choose folder">
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <path d="M2 4h12v8H2Z" />
        </svg>
      </IconActionButton>
    </MixRelayProvider>,
  );
  const button = screen.getByRole('button', { name: 'Choose folder' });
  fireEvent.pointerMove(document.body);
  fireEvent.pointerOver(button);
  await act(async () => vi.advanceTimersByTimeAsync(motion.tooltipDelay + 200));
  expect(screen.getByRole('tooltip')).toHaveTextContent('Choose folder');
  expect(button).toHaveAccessibleDescription('Choose folder');
  fireEvent.pointerOut(button);
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
});

it('has one keyboard focus owner, exposes the label on focus and preserves Enter and Space activation', async () => {
  const click = vi.fn();
  render(
    <MixRelayProvider>
      <IconActionButton label="Retry connection" onClick={click}>
        ↻
      </IconActionButton>
      <button type="button">Next action</button>
    </MixRelayProvider>,
  );
  const user = userEvent.setup();
  await user.tab();
  const button = screen.getByRole('button', { name: 'Retry connection' });
  expect(button).toHaveFocus();
  expect(await screen.findByRole('tooltip')).toHaveTextContent('Retry connection');
  await user.keyboard('{Enter}');
  await user.keyboard(' ');
  expect(click).toHaveBeenCalledTimes(2);
  await user.tab();
  expect(screen.getByRole('button', { name: 'Next action' })).toHaveFocus();
});

it('skips a disabled icon during Tab navigation and blocks activation without adding a wrapper stop', async () => {
  const click = vi.fn();
  render(
    <MixRelayProvider>
      <IconActionButton label="Unavailable action" disabled onClick={click}>
        ↻
      </IconActionButton>
      <button type="button">Next action</button>
    </MixRelayProvider>,
  );
  const user = userEvent.setup();
  await user.tab();
  expect(screen.getByRole('button', { name: 'Next action' })).toHaveFocus();
  await user.click(screen.getByRole('button', { name: 'Unavailable action' }));
  expect(click).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Unavailable action' })).toBeDisabled();
});
