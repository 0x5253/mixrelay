import { act, render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { NodeTransportRow } from './NodeTransportRow';
import { StatusBadge } from './StatusBadge';
import { StatusIndicator } from './StatusIndicator';
import { Tooltip } from './Tooltip';

it('shows a status badge tone without accepting caller styling', () => {
  render(
    <MixRelayProvider>
      <StatusBadge tone="warning">Paused</StatusBadge>
      <StatusBadge>Stopped</StatusBadge>
    </MixRelayProvider>,
  );
  expect(screen.getByText('Paused')).toHaveAttribute('data-tone', 'warning');
  expect(screen.getByText('Stopped')).toHaveAttribute('data-tone', 'neutral');
});

it('names a status marker without adding a Tab stop and defaults to the neutral tone', () => {
  render(
    <MixRelayProvider>
      <StatusIndicator label="Audio session running" tone="positive" position="left" />
      <StatusIndicator label="Unknown" />
    </MixRelayProvider>,
  );
  const running = screen.getByRole('img', { name: 'Audio session running' });
  expect(running).toHaveAttribute('data-tone', 'positive');
  expect(running).not.toHaveAttribute('tabindex', '0');
  expect(screen.getByRole('img', { name: 'Unknown' })).toHaveAttribute('data-tone', 'neutral');
});

it('describes the transport value once and omits an empty description', () => {
  const { rerender } = render(
    <NodeTransportRow
      value="00:00:01.000"
      label="Recording time"
      state="Recording"
      description="Disk 92%"
    >
      <button type="button">Stop</button>
    </NodeTransportRow>,
  );
  const value = screen.getByRole('status', { name: 'Recording time' });
  expect(value).toHaveAttribute('aria-description', 'Recording · Disk 92%');
  expect(value).toHaveAttribute('title', 'Recording · Disk 92%');
  rerender(
    <NodeTransportRow value="00:00:00.000" label="Recording time">
      <button type="button">Record</button>
    </NodeTransportRow>,
  );
  expect(value).not.toHaveAttribute('aria-description');
  expect(value).not.toHaveAttribute('title');
});

it('renders the readout tooltip form with the shared tooltip owner', async () => {
  render(
    <MixRelayProvider>
      <Tooltip label="CH 3" kind="readout" opened nonInteractive>
        <span role="img" aria-label="Main 3, CH 3" />
      </Tooltip>
      <Tooltip label="Delete graph" opened>
        <button type="button">Delete</button>
      </Tooltip>
    </MixRelayProvider>,
  );
  await act(async () => {});
  const tooltips = screen.getAllByRole('tooltip');
  const readout = tooltips.find((tooltip) => tooltip.textContent === 'CH 3');
  const label = tooltips.find((tooltip) => tooltip.textContent === 'Delete graph');
  expect(readout?.className).not.toBe(label?.className);
  expect(readout?.className).toContain(label?.className);
});
