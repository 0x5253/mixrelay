import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioGraphCanvas } from './AudioGraphCanvas';
import type { AudioGraphModel } from './model';

// Uses the real React Flow: its deletion keys listen on the document and track pressed keys.
function model(): AudioGraphModel {
  return {
    revision: 1,
    nodes: [
      {
        id: 'source',
        position: { x: 0, y: 0 },
        inputPorts: [],
        outputPorts: [{ id: 'out', label: 'Source output' }],
        appearance: { category: 'Source', detail: 'App', icon: 'S', title: 'Source' },
      },
      {
        id: 'sink',
        position: { x: 300, y: 0 },
        inputPorts: [{ id: 'in', label: 'Sink input' }],
        outputPorts: [],
        appearance: { category: 'Output', detail: 'OBS', icon: 'O', title: 'Sink' },
      },
    ],
    edges: [],
  };
}

it('deletes the selection only while focus is in the graph, and still after focus returns', async () => {
  const onModelChange = vi.fn();
  const { container } = render(
    <MixRelayProvider>
      <button type="button">Outside</button>
      <AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={onModelChange} />
    </MixRelayProvider>,
  );
  // jsdom has no layout, so React Flow keeps nodes visually hidden; query the DOM directly.
  await waitFor(() =>
    expect(container.querySelector('.react-flow__node[data-id="sink"]')).not.toBeNull(),
  );
  const node = container.querySelector('.react-flow__node[data-id="sink"]') as HTMLElement;
  fireEvent.click(node);
  await waitFor(() => expect(node).toHaveClass('selected'));
  const outside = screen.getByRole('button', { name: 'Outside' });
  // Shift+Tab out of the graph: Shift goes down inside and comes up outside.
  act(() => node.focus());
  fireEvent.keyDown(node, { key: 'Shift', code: 'ShiftLeft', shiftKey: true });
  act(() => outside.focus());
  fireEvent.keyUp(outside, { key: 'Shift', code: 'ShiftLeft' });
  fireEvent.keyDown(outside, { key: 'Delete', code: 'Delete' });
  fireEvent.keyUp(outside, { key: 'Delete', code: 'Delete' });
  await act(async () => {});
  expect(onModelChange).not.toHaveBeenCalled();
  act(() => node.focus());
  fireEvent.keyDown(node, { key: 'Delete', code: 'Delete' });
  fireEvent.keyUp(node, { key: 'Delete', code: 'Delete' });
  await waitFor(() => expect(onModelChange).toHaveBeenCalledOnce());
  expect(onModelChange.mock.calls[0]?.[0].nodes.map((item: { id: string }) => item.id)).toEqual([
    'source',
  ]);
});
