import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { AudioGraphModel } from './model';
import { PortSelectionFeedback } from './PortSelectionFeedback';

const model: AudioGraphModel = {
  revision: 1,
  edges: [],
  nodes: [
    {
      id: 'source',
      inputPorts: [],
      position: { x: 0, y: 0 },
      appearance: { category: 'input', title: 'Source', icon: 'audio' },
      outputPorts: [
        { id: 'default', label: 'Default' },
        { id: 'explicit-4', label: 'Explicit 4', channelIndex: 3 },
        { id: 'explicit-5', label: 'Explicit 5', channelIndex: 4 },
        { id: 'explicit-7', label: 'Explicit 7', channelIndex: 6 },
      ],
    },
  ],
};

const port = (portId: string, nodeId = 'source') => ({ nodeId, portId, side: 'output' as const });

describe('selected channel feedback', () => {
  it.each([
    { position: null, ports: [port('default')] },
    { position: { x: 10, y: 20 }, ports: [] },
    { position: { x: 10, y: 20 }, ports: [port('absent')] },
    { position: { x: 10, y: 20 }, ports: [port('default', 'absent')] },
  ])('does not announce nonexistent or inactive channel selections %j', (selection) => {
    render(<PortSelectionFeedback model={model} {...selection} />);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('announces distinct sparse and consecutive channel ranges using explicit indices', () => {
    render(
      <PortSelectionFeedback
        model={model}
        position={{ x: 10, y: 20 }}
        ports={[
          port('explicit-7'),
          port('explicit-5'),
          port('default'),
          port('explicit-4'),
          port('explicit-4'),
        ]}
      />,
    );
    expect(screen.getByRole('status')).toHaveTextContent('CH 1, 4–5, 7');
    expect(screen.getByRole('status')).toHaveAttribute('aria-live', 'polite');
  });

  it('announces a single selected channel without a spurious range', () => {
    render(
      <PortSelectionFeedback model={model} position={{ x: 10, y: 20 }} ports={[port('default')]} />,
    );
    expect(screen.getByRole('status')).toHaveTextContent('CH 1');
  });
});

it('announces a contiguous final range rather than separate selected channel labels', () => {
  render(
    <PortSelectionFeedback
      model={model}
      position={{ x: 10, y: 20 }}
      ports={[port('explicit-4'), port('explicit-5')]}
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent('CH 4–5');
});
