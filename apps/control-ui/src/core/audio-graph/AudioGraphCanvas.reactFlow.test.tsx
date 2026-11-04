import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import type { AudioGraphModel } from './model';

// These tests use the real React Flow, so they share one file and its one-time warm-up.

// A render-counting card guards against context values or props that change identity on
// every canvas render and so re-render every node.
const counter = vi.hoisted(() => ({ cards: [] as string[] }));
vi.mock('../ui/AudioNodeCard', async (original) => {
  const actual = await original<typeof import('../ui/AudioNodeCard')>();
  return {
    ...actual,
    AudioNodeCard: (props: Parameters<typeof actual.AudioNodeCard>[0]) => {
      counter.cards.push(props.title);
      return actual.AudioNodeCard(props);
    },
  };
});
const { AudioGraphCanvas } = await import('./AudioGraphCanvas');

// A few nodes show the identity contract; more would only repeat each render.
const nodeCount = 4;

function graph(firstGainDb: number): AudioGraphModel {
  return {
    revision: 1,
    edges: [],
    nodes: Array.from({ length: nodeCount }, (_, index) => ({
      id: `n${index}`,
      position: { x: index * 260, y: 0 },
      inputPorts: [{ id: 'in', label: 'In' }],
      outputPorts: [{ id: 'out', label: 'Out' }],
      appearance: {
        category: 'Processor',
        icon: 'FX',
        title: `Node ${index}`,
        gainControl: { label: 'Gain', valueDb: index === 0 ? firstGainDb : 0 },
      },
    })),
  };
}

const callbacks = {
  onModelChange: vi.fn(),
  onNodeGainChange: vi.fn(),
  renderNodeControls: () => null,
  renderNodeContent: () => null,
};

function canvas(model: AudioGraphModel) {
  return (
    <MixRelayProvider>
      <AudioGraphCanvas ariaLabel="Graph" model={model} {...callbacks} />
    </MixRelayProvider>
  );
}

async function settle() {
  for (let pass = 0; pass < 3; pass += 1) await act(async () => {});
}

it('re-renders only the node whose data changed', async () => {
  const view = render(canvas(graph(0)));
  await settle();
  expect(new Set(counter.cards).size).toBe(nodeCount);
  counter.cards = [];
  // An equal model, rebuilt as a new object (as presenters do on every status poll).
  view.rerender(canvas(graph(0)));
  await settle();
  expect(counter.cards).toEqual([]);
  view.rerender(canvas(graph(-6)));
  await settle();
  expect(counter.cards).toEqual(['Node 0']);
});

function sourceAndSink(): AudioGraphModel {
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

// React Flow's deletion keys listen on the document and track pressed keys.
it('deletes the selection only while focus is in the graph, and still after focus returns', async () => {
  const onModelChange = vi.fn();
  const { container } = render(
    <MixRelayProvider>
      <button type="button">Outside</button>
      <AudioGraphCanvas ariaLabel="Graph" model={sourceAndSink()} onModelChange={onModelChange} />
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
