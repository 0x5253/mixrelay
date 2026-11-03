import { act, render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import type { AudioGraphModel } from './model';

// Real React Flow with a render-counting card: guards against context values or props that
// change identity on every canvas render and so re-render every node.
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

function graph(firstGainDb: number): AudioGraphModel {
  return {
    revision: 1,
    edges: [],
    nodes: Array.from({ length: 30 }, (_, index) => ({
      id: `n${index}`,
      position: { x: (index % 6) * 260, y: Math.floor(index / 6) * 200 },
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
  expect(new Set(counter.cards).size).toBe(30);
  counter.cards = [];
  // An equal model, rebuilt as a new object (as presenters do on every status poll).
  view.rerender(canvas(graph(0)));
  await settle();
  expect(counter.cards).toEqual([]);
  view.rerender(canvas(graph(-6)));
  await settle();
  expect(counter.cards).toEqual(['Node 0']);
});
