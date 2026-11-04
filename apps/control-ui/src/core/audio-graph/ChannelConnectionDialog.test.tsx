import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { ChannelConnectionDialog } from './ChannelConnectionDialog';
import type { AudioGraphModel, AudioGraphNode } from './model';

function node(id: string, title: string, inputs: number, outputs: number): AudioGraphNode {
  const ports = (prefix: string, label: string, count: number) =>
    Array.from({ length: count }, (_, index) => ({ id: `${prefix}${index + 1}`, label }));
  return {
    id,
    position: { x: 0, y: 0 },
    inputPorts: ports('in', `${title} input`, inputs),
    outputPorts: ports('out', `${title} output`, outputs),
    appearance: { category: 'Test', detail: id, icon: 'T', title },
  };
}

function graph(): AudioGraphModel {
  return {
    revision: 1,
    nodes: [
      node('app', 'Music', 0, 4),
      node('mon-a', 'Monitor', 2, 0),
      node('mon-b', 'Monitor', 3, 0),
      node('meter', 'Meter', 0, 2),
    ],
    edges: [],
  };
}

function renderDialog(props: Partial<Parameters<typeof ChannelConnectionDialog>[0]> = {}) {
  const onConnect = vi.fn();
  const onCancel = vi.fn();
  const view = render(
    <ChannelConnectionDialog
      model={graph()}
      sourceNodeId="app"
      onCancel={onCancel}
      onConnect={onConnect}
      {...props}
    />,
    { wrapper: MixRelayProvider },
  );
  return { ...view, onConnect, onCancel };
}

function choose(field: RegExp, option: string) {
  fireEvent.click(screen.getByRole('button', { name: field }));
  fireEvent.click(screen.getByRole('option', { name: option }));
}

// Planning rules are unit-tested in channelConnectionForm.test.ts; these cover what the
// dialog shows, what it submits, and where focus starts.
const preview = () => within(screen.getByRole('list', { name: 'Preview' }));

it('previews every pair and skipped channel, then commits only the accepted pairs', () => {
  const { onConnect } = renderDialog();
  const connect = screen.getByRole('button', { name: 'Connect' });
  expect(connect).toBeDisabled();
  fireEvent.submit(connect.closest('form') as HTMLFormElement);
  expect(onConnect).not.toHaveBeenCalled();
  choose(/Destination$/, 'Monitor (2)');
  choose(/First input channel$/, 'Monitor input, CH 2');
  expect(screen.getByRole('status')).toHaveTextContent('Will connect: 2 · Skipped: 2');
  expect(
    preview()
      .getAllByRole('listitem')
      .map((item) => item.textContent),
  ).toEqual([
    'CH 1 → CH 2',
    'CH 2 → CH 3',
    'CH 3: No matching input channel',
    'CH 4: No matching input channel',
  ]);
  choose(/Number of channels$/, '2');
  expect(screen.getByRole('status')).toHaveTextContent('Will connect: 2 · Skipped: 0');
  fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
  expect(onConnect).toHaveBeenCalledExactlyOnceWith([
    { sourceNodeId: 'app', sourcePortId: 'out1', targetNodeId: 'mon-b', targetPortId: 'in2' },
    { sourceNodeId: 'app', sourcePortId: 'out2', targetNodeId: 'mon-b', targetPortId: 'in3' },
  ]);
});

it('keeps a sparse canvas selection until the range is chosen explicitly', () => {
  const { onConnect } = renderDialog({ sourcePortIds: ['out2', 'out4'] });
  expect(screen.getByRole('dialog')).toHaveTextContent('Selected channels: CH 2, CH 4');
  expect(screen.queryByRole('button', { name: /First output channel$/ })).toBeNull();
  choose(/Destination$/, 'Monitor (2)');
  expect(
    preview()
      .getAllByRole('listitem')
      .map((item) => item.textContent),
  ).toEqual(['CH 2 → CH 1', 'CH 4 → CH 3']);
  fireEvent.click(screen.getByRole('button', { name: 'Use a channel range' }));
  expect(screen.getByRole('button', { name: /First output channel$/ })).toHaveTextContent(
    'Music output, CH 2',
  );
  expect(screen.getByRole('button', { name: /Number of channels$/ })).toHaveTextContent('3');
  fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
  expect(onConnect.mock.calls[0]?.[0]).toHaveLength(3);
});

it('explains when no other node accepts input and cancels without connecting', () => {
  const lonely: AudioGraphModel = { revision: 1, nodes: [node('app', 'Music', 0, 2)], edges: [] };
  const { onCancel, onConnect } = renderDialog({ model: lonely });
  expect(screen.getByText('No other node has input channels.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(onCancel).toHaveBeenCalledOnce();
  expect(onConnect).not.toHaveBeenCalled();
});

it('shows a vanished first output channel as unselected, matching what would be submitted', () => {
  const { rerender } = renderDialog();
  choose(/First output channel$/, 'Music output, CH 4');
  choose(/Destination$/, 'Monitor (1)');
  const shrunk = graph();
  shrunk.nodes[0] = node('app', 'Music', 0, 2);
  rerender(
    <ChannelConnectionDialog
      model={shrunk}
      sourceNodeId="app"
      onCancel={vi.fn()}
      onConnect={vi.fn()}
    />,
  );
  expect(screen.getByRole('button', { name: /First output channel$/ })).toHaveTextContent(
    'Choose a channel',
  );
  expect(screen.getByRole('button', { name: /Number of channels$/ })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
  choose(/First output channel$/, 'Music output, CH 2');
  expect(screen.getByRole('status')).toHaveTextContent('Will connect: 1 · Skipped: 0');
});

it('focuses the first field: the source channel, or the destination for a kept selection', async () => {
  const { unmount } = renderDialog();
  await waitFor(() =>
    expect(screen.getByRole('button', { name: /First output channel$/ })).toHaveFocus(),
  );
  unmount();
  renderDialog({ sourcePortIds: ['out1'] });
  await waitFor(() => expect(screen.getByRole('button', { name: /Destination$/ })).toHaveFocus());
});
