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

const preview = () => within(screen.getByRole('list', { name: 'Preview' }));

it('starts from every output channel and connects nothing until a destination is chosen', () => {
  const { onConnect } = renderDialog();
  const dialog = screen.getByRole('dialog', { name: 'Connect channels' });
  expect(dialog).toHaveTextContent('From Music');
  expect(screen.getByRole('button', { name: /First output channel$/ })).toHaveTextContent(
    'Music output, CH 1',
  );
  expect(screen.getByRole('button', { name: /Number of channels$/ })).toHaveTextContent('4');
  expect(screen.getByRole('status')).toHaveTextContent(
    'Choose a destination and channels to preview the connections.',
  );
  const connect = screen.getByRole('button', { name: 'Connect' });
  expect(connect).toBeDisabled();
  fireEvent.submit(connect.closest('form') as HTMLFormElement);
  expect(onConnect).not.toHaveBeenCalled();
});

it('names repeated destinations distinctly and skips nodes without inputs', () => {
  renderDialog();
  fireEvent.click(screen.getByRole('button', { name: /Destination$/ }));
  expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual([
    'Monitor (1)',
    'Monitor (2)',
  ]);
});

it('previews every pair and skipped channel, then commits only the accepted pairs', () => {
  const { onConnect } = renderDialog();
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

it('clamps the channel count when a later first channel leaves fewer outputs', () => {
  renderDialog();
  choose(/First output channel$/, 'Music output, CH 3');
  expect(screen.getByRole('button', { name: /Number of channels$/ })).toHaveTextContent('2');
  fireEvent.click(screen.getByRole('button', { name: /Number of channels$/ }));
  expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual(['1', '2']);
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

it('revalidates on submit against the graph as it is now', () => {
  const { onConnect, rerender } = renderDialog();
  choose(/Destination$/, 'Monitor (1)');
  choose(/Number of channels$/, '2');
  const changed = graph();
  changed.edges = [
    {
      id: 'e',
      sourceNodeId: 'app',
      sourcePortId: 'out1',
      targetNodeId: 'mon-a',
      targetPortId: 'in1',
    },
  ];
  rerender(
    <ChannelConnectionDialog
      model={changed}
      sourceNodeId="app"
      onCancel={vi.fn()}
      onConnect={onConnect}
    />,
  );
  expect(
    preview()
      .getAllByRole('listitem')
      .map((item) => item.textContent),
  ).toEqual(['CH 2 → CH 2', 'CH 1: Already connected']);
  fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
  expect(onConnect).toHaveBeenCalledExactlyOnceWith([
    { sourceNodeId: 'app', sourcePortId: 'out2', targetNodeId: 'mon-a', targetPortId: 'in2' },
  ]);
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

it('stays usable when the source node disappears while open', () => {
  const { rerender, onConnect } = renderDialog();
  rerender(
    <ChannelConnectionDialog
      model={{ revision: 2, nodes: graph().nodes.slice(1), edges: [] }}
      sourceNodeId="app"
      onCancel={vi.fn()}
      onConnect={onConnect}
    />,
  );
  expect(screen.getByRole('dialog')).toHaveTextContent('From app');
  expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
});

it('asks again for a chosen input channel that disappears instead of moving the cables', () => {
  const { rerender, onConnect } = renderDialog();
  choose(/Destination$/, 'Monitor (2)');
  choose(/First input channel$/, 'Monitor input, CH 3');
  const shrunk = graph();
  shrunk.nodes[2] = node('mon-b', 'Monitor', 2, 0);
  rerender(
    <ChannelConnectionDialog
      model={shrunk}
      sourceNodeId="app"
      onCancel={vi.fn()}
      onConnect={onConnect}
    />,
  );
  expect(screen.getByRole('button', { name: /First input channel$/ })).toHaveTextContent(
    'Choose a channel',
  );
  expect(screen.getByRole('status')).toHaveTextContent(
    'Choose a destination and channels to preview the connections.',
  );
  expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
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

it('falls back to choosing a range when every selected channel disappears', () => {
  const { rerender } = renderDialog({ sourcePortIds: ['out4'] });
  const shrunk = graph();
  shrunk.nodes[0] = node('app', 'Music', 0, 2);
  rerender(
    <ChannelConnectionDialog
      model={shrunk}
      sourceNodeId="app"
      sourcePortIds={['out4']}
      onCancel={vi.fn()}
      onConnect={vi.fn()}
    />,
  );
  expect(screen.queryByText(/Selected channels/)).toBeNull();
  expect(screen.getByRole('button', { name: /First output channel$/ })).toHaveTextContent(
    'Choose a channel',
  );
});

it('offers a boundary self-connection that dragging allows, but not other self-loops', () => {
  const device = node('virtual', 'Virtual device', 1, 1);
  device.inputPorts[0] = { boundary: 'sink', id: 'to_os', label: 'To system' };
  device.outputPorts[0] = { boundary: 'source', id: 'from_os', label: 'From system' };
  const { onConnect } = renderDialog({
    model: { revision: 1, nodes: [device, node('fx', 'Effect', 1, 1)], edges: [] },
    sourceNodeId: 'virtual',
  });
  choose(/Destination$/, 'Virtual device');
  fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
  expect(onConnect).toHaveBeenCalledExactlyOnceWith([
    {
      sourceNodeId: 'virtual',
      sourcePortId: 'from_os',
      targetNodeId: 'virtual',
      targetPortId: 'to_os',
    },
  ]);
  renderDialog({
    model: { revision: 1, nodes: [node('fx', 'Effect', 1, 1), device], edges: [] },
    sourceNodeId: 'fx',
  });
  fireEvent.click(screen.getAllByRole('button', { name: /Destination$/ }).at(-1) as HTMLElement);
  expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual([
    'Virtual device',
  ]);
});

it('opens without channels for a source that vanished before it opened', () => {
  renderDialog({ sourceNodeId: 'gone' });
  expect(screen.getByRole('dialog')).toHaveTextContent('From gone');
  expect(screen.getByRole('button', { name: /First output channel$/ })).toHaveTextContent(
    'Choose a channel',
  );
  expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
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
