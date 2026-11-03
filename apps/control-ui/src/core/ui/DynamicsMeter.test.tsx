import { act, render, screen } from '@testing-library/react';
import { renderToString } from 'react-dom/server';
import { expect, it } from 'vitest';
import { AudioMeterProvider } from '../audio-meter/AudioMeterProvider';
import { AudioMeterStore } from '../audio-meter/AudioMeterStore';
import { DynamicsMeter } from './DynamicsMeter';

const contract = {
  nodeId: 'comp',
  portId: 'audio',
  thresholdId: 'thresholdDb',
  inputLabel: 'Input',
  outputLabel: 'Output',
  reductionLabel: 'Gain reduction',
  unavailableLabel: 'Waiting for audio telemetry',
};
const controls = [
  {
    id: 'thresholdDb',
    label: 'Threshold',
    value: -24,
    minimum: -60,
    maximum: 0,
    step: 0.5,
    suffix: 'dB',
  },
];
function snapshot(
  inputPeaks = [1, 0.5],
  gainReductionDb = 13.5,
  thresholdDb = -18,
  peaks = [0.8414, 0.4207],
) {
  return {
    protocolVersion: 1 as const,
    revision: 1,
    nodes: [
      {
        nodeId: 'comp',
        portId: 'audio',
        peaks,
        dynamics: { inputPeaks, gainReductionDb, thresholdDb },
      },
    ],
  };
}
it('binds native input/output, pre-makeup gain reduction and the applied threshold rather than the requested control', () => {
  const store = new AudioMeterStore();
  store.update(snapshot());
  const view = render(
    <AudioMeterProvider store={store}>
      <DynamicsMeter contract={contract} controls={controls} />
    </AudioMeterProvider>,
  );
  expect(screen.getByRole('meter', { name: 'Gain reduction' })).toHaveAttribute(
    'aria-valuenow',
    '13.5',
  );
  expect(screen.getByRole('meter', { name: 'Input L' })).toHaveAttribute(
    'aria-valuetext',
    '0.0 dB',
  );
  expect(screen.getByRole('meter', { name: 'Input R' })).toHaveAttribute(
    'aria-valuetext',
    '-6.0 dB',
  );
  expect(screen.getByRole('meter', { name: 'Output L' })).toHaveAttribute(
    'aria-valuetext',
    '-1.5 dB',
  );
  expect(view.container.querySelector('[data-threshold-db]')).toHaveStyle({ left: '70%' });
  expect(view.container.querySelectorAll('[data-threshold-db]')).toHaveLength(2);
  for (const channel of ['L', 'R']) {
    expect(
      screen.getByRole('meter', { name: `Input ${channel}` }).querySelector('[data-threshold-db]'),
    ).toHaveStyle({ left: '70%' });
    expect(
      screen.getByRole('meter', { name: `Output ${channel}` }).querySelector('[data-threshold-db]'),
    ).toBeNull();
  }
  expect(screen.getAllByRole('meter')).toHaveLength(5);
  expect(screen.getByText('Threshold -18.0 dB')).toBeInTheDocument();
  act(() => store.update(snapshot([0.01, 0.005], 0, -24, [0.01, 0.005])));
  expect(screen.getByRole('meter', { name: 'Gain reduction' })).toHaveAttribute(
    'aria-valuenow',
    '0',
  );
  expect(screen.getByText('Threshold -24.0 dB')).toBeInTheDocument();
  expect(view.container.querySelector('[data-threshold-db]')).toHaveStyle({ left: '60%' });
  act(() => store.clear());
  expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  expect(screen.queryByText(contract.unavailableLabel)).not.toBeInTheDocument();
  expect(
    screen.getByRole('img', { name: `Gain reduction: ${contract.unavailableLabel}` }),
  ).toBeInTheDocument();
});
it('keeps quiet finite input distinct from digital silence and displays actual reduction beyond the rail range', () => {
  const store = new AudioMeterStore();
  store.update(snapshot([10 ** (-70 / 20), 0], 40, -60, [2, 0]));
  const view = render(
    <AudioMeterProvider store={store}>
      <DynamicsMeter contract={contract} controls={controls} bypassed disabled />
    </AudioMeterProvider>,
  );
  expect(screen.getByRole('meter', { name: 'Input L' })).toHaveAttribute(
    'aria-valuetext',
    '-70.0 dB',
  );
  expect(screen.getByRole('meter', { name: 'Input R' })).toHaveAttribute('aria-valuetext', '−∞ dB');
  expect(screen.getByRole('meter', { name: 'Output L' })).toHaveAttribute(
    'aria-valuetext',
    '6.0 dB',
  );
  expect(screen.getByRole('meter', { name: 'Gain reduction' })).toHaveAttribute(
    'aria-valuenow',
    '24',
  );
  expect(screen.getByRole('meter', { name: 'Gain reduction' })).toHaveAttribute(
    'aria-valuetext',
    '40.0 dB',
  );
  expect(view.container.querySelector('[data-threshold-db]')).toHaveStyle({ left: '0%' });
  expect(view.container.querySelector('[data-bypassed][data-disabled]')).toBeInTheDocument();
  // Read-only meter presentation creates no extra keyboard focus owner.
  expect(view.container.querySelector('[tabindex]')).toBeNull();
});
it('uses channel indices for nonstereo layouts and clears subscriptions when meter identity changes', () => {
  const store = new AudioMeterStore();
  store.update(snapshot([0.1], 0, -18, [0.1]));
  const view = render(
    <AudioMeterProvider store={store}>
      <DynamicsMeter contract={contract} controls={controls} />
    </AudioMeterProvider>,
  );
  expect(screen.getByRole('meter', { name: 'Input 1' })).toBeInTheDocument();
  view.rerender(
    <AudioMeterProvider store={store}>
      <DynamicsMeter contract={{ ...contract, nodeId: 'other' }} controls={controls} />
    </AudioMeterProvider>,
  );
  expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  act(() => store.update(snapshot()));
  expect(screen.queryByText(contract.unavailableLabel)).not.toBeInTheDocument();
  expect(
    screen.getByRole('img', { name: `Gain reduction: ${contract.unavailableLabel}` }),
  ).toBeInTheDocument();
});
it('shows pending without inventing native values, rejects unresolved control contracts and omits stale server meters', () => {
  const view = render(<DynamicsMeter contract={contract} controls={controls} />);
  expect(screen.queryByText(contract.unavailableLabel)).not.toBeInTheDocument();
  expect(
    screen.getByRole('img', { name: `Gain reduction: ${contract.unavailableLabel}` }),
  ).toBeInTheDocument();
  view.rerender(<DynamicsMeter contract={contract} controls={[]} />);
  expect(view.container).toBeEmptyDOMElement();
  const store = new AudioMeterStore();
  store.update(snapshot());
  const server = renderToString(
    <AudioMeterProvider store={store}>
      <DynamicsMeter contract={contract} controls={controls} />
    </AudioMeterProvider>,
  );
  expect(server).toContain(`Gain reduction: ${contract.unavailableLabel}`);
  expect(server).not.toContain('role="meter"');
});

it('keeps the same banks for every configured channel without presenting absent readings as measured zero', () => {
  const view = render(
    <DynamicsMeter contract={{ ...contract, channels: 4 }} controls={controls} />,
  );
  expect(screen.getAllByRole('img')).toHaveLength(9);
  expect(screen.getAllByText('—')).toHaveLength(9);
  expect(screen.queryByText(contract.unavailableLabel)).not.toBeInTheDocument();
  expect(
    screen.getByRole('img', { name: `Input 4: ${contract.unavailableLabel}` }),
  ).not.toHaveAttribute('aria-valuenow');
  expect(screen.getByText('Threshold -24.0 dB')).toBeInTheDocument();
  expect(view.container.querySelectorAll('[data-threshold-db]')).toHaveLength(4);
  expect(view.container.querySelector('[tabindex]')).toBeNull();
});
