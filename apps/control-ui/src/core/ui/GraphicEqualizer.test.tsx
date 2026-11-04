import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard } from './AudioNodeCard';
import { frequencyPosition, gainPosition } from './equalizerResponse';
import { GraphicEqualizer, type GraphicEqualizerContract } from './GraphicEqualizer';
import type { ParameterFieldControl } from './ParameterField';

const contract: GraphicEqualizerContract = {
  sampleRate: 48000,
  bands: [
    { id: 'low', label: 'Low', frequencyId: 'lowFrequencyHz', gainId: 'lowDb', qId: 'lowQ' },
    { id: 'mid', label: 'Mid', frequencyId: 'midFrequencyHz', gainId: 'midDb', qId: 'midQ' },
    { id: 'high', label: 'High', frequencyId: 'highFrequencyHz', gainId: 'highDb', qId: 'highQ' },
  ],
};
const controls: ParameterFieldControl[] = contract.bands.flatMap((band, index) => [
  {
    id: band.frequencyId,
    label: 'Frequency',
    value: [150, 1000, 6000][index] as number,
    minimum: 20,
    maximum: 20000,
    step: 1,
    suffix: 'Hz',
    scale: 'logarithmic',
  },
  { id: band.gainId, label: 'Gain', value: 0, minimum: -24, maximum: 24, step: 0.1, suffix: 'dB' },
  {
    id: band.qId,
    label: 'Q',
    value: Number(Math.SQRT1_2.toFixed(3)),
    minimum: 0.1,
    maximum: 10,
    step: 0.001,
    suffix: '',
  },
]);
function setup(props: Partial<Parameters<typeof GraphicEqualizer>[0]> = {}) {
  const change = vi.fn();
  const batch = vi.fn();
  const view = render(
    <MixRelayProvider>
      <GraphicEqualizer
        contract={contract}
        controls={controls}
        onChange={change}
        onParametersChange={batch}
        {...props}
      />
    </MixRelayProvider>,
  );
  const plot = view.container.querySelector('svg')?.parentElement as HTMLDivElement;
  if (plot) {
    plot.setPointerCapture = vi.fn();
    plot.hasPointerCapture = vi.fn(() => true);
    plot.releasePointerCapture = vi.fn();
    plot.getBoundingClientRect = vi.fn(() => ({
      left: 10,
      top: 20,
      width: 300,
      height: 120,
      right: 310,
      bottom: 140,
      x: 10,
      y: 20,
      toJSON: () => {},
    }));
  }
  return { ...view, change, batch, plot };
}
// Rendering nine controls and exercising selection can exceed 5 s on Intel CI.
it('shows all nine exact values, distinct band names, vector axes and native selectable handles', {
  timeout: 15_000,
}, () => {
  const { container } = setup();
  expect(screen.getAllByRole('textbox')).toHaveLength(9);
  expect(screen.queryByRole('slider')).toBeNull();
  expect(screen.getByRole('textbox', { name: '1 Low Hz' })).toHaveValue('150');
  expect(screen.getByRole('textbox', { name: '3 High Q' })).toHaveValue('0.707');
  expect(container.querySelector('svg')).toHaveAttribute('viewBox', '0 0 300 120');
  expect(container.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  expect(screen.getByText('5k')).toBeInTheDocument();
  expect(screen.getByText('20k')).toBeInTheDocument();
  const mid = screen.getByRole('button', { name: '2 Mid' });
  fireEvent.click(mid);
  expect(mid).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByRole('button', { name: /2 Mid, 1000 Hz/ })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  fireEvent.focus(screen.getByRole('textbox', { name: '3 High Q' }));
  expect(screen.getByRole('button', { name: '3 High' })).toHaveAttribute('aria-pressed', 'true');
  fireEvent.click(screen.getByRole('button', { name: /1 Low, 150 Hz/ }));
  expect(screen.getByRole('button', { name: '1 Low' })).toHaveAttribute('aria-pressed', 'true');
});
// Each arrow's step and range are unit-tested in equalizerResponse.test.ts.
it('executes graph arrows through the same parameters as exact numeric entry', () => {
  const { change } = setup();
  const mid = screen.getByRole('button', { name: /2 Mid, 1000 Hz/ });
  fireEvent.focus(mid);
  fireEvent.keyDown(mid, { key: 'ArrowRight' });
  expect(change).toHaveBeenLastCalledWith('midFrequencyHz', 1001);
  fireEvent.keyDown(mid, { key: 'Tab' });
  expect(change).toHaveBeenCalledOnce();
  const input = screen.getByRole('textbox', { name: '2 Mid Q' });
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value: '2.003' } });
  fireEvent.blur(input);
  expect(change).toHaveBeenLastCalledWith('midQ', 2.003);
});
it('delivers a frequency/gain drag as one atomic batch, retaining selection and pointer ownership', () => {
  const { plot, batch, change } = setup();
  const mid = screen.getByRole('button', { name: /2 Mid, 1000 Hz/ });
  fireEvent.pointerDown(plot, { button: 0, pointerId: 1, clientX: 40, clientY: 70 });
  expect(plot.setPointerCapture).not.toHaveBeenCalled();
  fireEvent.pointerDown(mid, { button: 2, pointerId: 1 });
  expect(plot.setPointerCapture).not.toHaveBeenCalled();
  fireEvent.pointerDown(mid, { button: 0, pointerId: 1 });
  expect(mid).toHaveFocus();
  expect(plot.setPointerCapture).toHaveBeenCalledWith(1);
  fireEvent.pointerDown(mid, { button: 0, pointerId: 2 });
  expect(plot.setPointerCapture).toHaveBeenCalledOnce();
  fireEvent.pointerMove(plot, { pointerId: 2, clientX: 0, clientY: 0 });
  expect(batch).not.toHaveBeenCalled();
  fireEvent.pointerMove(plot, {
    pointerId: 1,
    clientX: 10 + frequencyPosition(2200),
    clientY: 20 + gainPosition(6),
  });
  expect(batch).toHaveBeenCalledExactlyOnceWith([
    { id: 'midFrequencyHz', value: 2200 },
    { id: 'midDb', value: 6 },
  ]);
  expect(change).not.toHaveBeenCalled();
  fireEvent.pointerUp(plot, { pointerId: 2 });
  expect(plot.releasePointerCapture).not.toHaveBeenCalled();
  fireEvent.pointerCancel(plot, { pointerId: 1 });
  expect(plot.releasePointerCapture).toHaveBeenCalledWith(1);
  fireEvent.pointerMove(plot, { pointerId: 1, clientX: 0, clientY: 0 });
  expect(batch).toHaveBeenCalledOnce();
});
it('keeps the graph editable while shared node Options hides and restores the nine-value table', () => {
  const change = vi.fn();
  const batch = vi.fn();
  render(
    <MixRelayProvider>
      <AudioNodeCard
        title="EQ"
        icon="FX"
        category="Processor"
        inputCount={2}
        outputCount={2}
        faceMode="compact"
        equalizer={contract}
        parameterControls={controls}
        onParameterChange={change}
        onParametersChange={batch}
        optionsLabel="Options"
      />
    </MixRelayProvider>,
  );
  const options = screen.getByRole('button', { name: (name) => name.endsWith('Options') });
  expect(options).toHaveAttribute('aria-expanded', 'false');
  expect(screen.queryByRole('textbox')).toBeNull();
  const high = screen.getByRole('button', { name: /3 High, 6000 Hz/ });
  fireEvent.keyDown(high, { key: 'ArrowRight' });
  expect(change).toHaveBeenLastCalledWith('highFrequencyHz', 6001);
  fireEvent.click(options);
  expect(options).toHaveAttribute('aria-expanded', 'true');
  expect(options.getAttribute('aria-controls')).toBeTruthy();
  expect(screen.getAllByRole('textbox')).toHaveLength(9);
  expect(screen.queryByRole('slider')).toBeNull();
  fireEvent.click(options);
  expect(screen.queryByRole('textbox')).toBeNull();
  expect(high).toBeInTheDocument();
});
it('releases graph capture on disabled transition, lost capture and unmount', () => {
  const { plot, batch, rerender, unmount } = setup();
  const handle = screen.getByRole('button', { name: /1 Low, 150 Hz/ });
  fireEvent.pointerDown(handle, { button: 0, pointerId: 5 });
  rerender(
    <GraphicEqualizer
      contract={contract}
      controls={controls}
      disabled
      onChange={vi.fn()}
      onParametersChange={batch}
    />,
  );
  expect(plot.releasePointerCapture).toHaveBeenCalledWith(5);
  expect(screen.getByRole('button', { name: /1 Low, 150 Hz/ })).toBeDisabled();
  const disabledPlot = screen.getByRole('button', { name: /1 Low, 150 Hz/ })
    .parentElement as HTMLDivElement;
  fireEvent.pointerDown(disabledPlot, { button: 0, pointerId: 6 });
  fireEvent.pointerMove(disabledPlot, { pointerId: 6, clientX: 100, clientY: 100 });
  fireEvent.keyDown(screen.getByRole('button', { name: /1 Low, 150 Hz/ }), { key: 'ArrowUp' });
  expect(batch).not.toHaveBeenCalled();
  unmount();
  const active = setup();
  fireEvent.pointerDown(screen.getByRole('button', { name: /1 Low, 150 Hz/ }), {
    button: 0,
    pointerId: 7,
  });
  active.unmount();
  expect(active.plot.releasePointerCapture).toHaveBeenCalledWith(7);
  const lost = setup();
  lost.plot.hasPointerCapture = vi.fn(() => false);
  fireEvent.pointerDown(screen.getByRole('button', { name: /1 Low, 150 Hz/ }), {
    button: 0,
    pointerId: 8,
  });
  fireEvent.lostPointerCapture(lost.plot);
  expect(lost.plot.releasePointerCapture).not.toHaveBeenCalled();
  fireEvent.pointerMove(lost.plot, { pointerId: 8, clientX: 100, clientY: 100 });
  expect(lost.batch).not.toHaveBeenCalled();
});
it('retains keyboard and exact entry without an atomic drag callback and calculates unknown-rate preview internally', () => {
  const { plot, batch } = setup({
    onParametersChange: undefined,
    contract: { ...contract, sampleRate: undefined },
    bypassed: true,
  });
  expect(screen.queryByText(/kHz/)).toBeNull();
  fireEvent.pointerDown(screen.getByRole('button', { name: /1 Low, 150 Hz/ }), {
    button: 0,
    pointerId: 1,
  });
  expect(plot.setPointerCapture).not.toHaveBeenCalled();
  expect(batch).not.toHaveBeenCalled();
});
it('omits incomplete bands instead of inventing values', () => {
  const { rerender } = setup({
    contract: { ...contract, sampleRate: undefined },
  });
  for (const id of ['lowFrequencyHz', 'lowDb', 'lowQ']) {
    rerender(
      <GraphicEqualizer
        contract={contract}
        controls={controls.filter((control) => control.id !== id)}
        onChange={vi.fn()}
      />,
    );
    expect(screen.queryByRole('button', { name: '1 Low' })).toBeNull();
    expect(screen.getAllByRole('textbox')).toHaveLength(6);
  }
  rerender(<GraphicEqualizer contract={{ bands: [] }} controls={[]} onChange={vi.fn()} />);
  expect(screen.queryByRole('textbox')).toBeNull();
});
it('shows a small pressed bypass header command instead of a full parameter selector and disables it for Off', () => {
  const change = vi.fn();
  const bypass = {
    id: 'bypass',
    label: 'Bypass',
    minimum: 0,
    maximum: 1,
    step: 1,
    suffix: '',
    value: 0,
    choices: [
      { value: 0, label: 'Processing' },
      { value: 1, label: 'Bypassed' },
    ],
  };
  const card = {
    title: 'EQ',
    icon: 'FX',
    category: 'Processor',
    inputCount: 2,
    outputCount: 2,
    faceMode: 'compact' as const,
    equalizer: contract,
    onParameterChange: change,
  };
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard {...card} parameterControls={[...controls, bypass]} />
    </MixRelayProvider>,
  );
  const button = screen.getByRole('button', {
    name: (name) => name.endsWith('Bypass: Processing'),
  });
  expect(button).toHaveAttribute('aria-pressed', 'false');
  expect(button.querySelector('svg')).toHaveAttribute('viewBox', '0 0 20 20');
  expect(screen.queryByRole('combobox')).toBeNull();
  fireEvent.click(button);
  expect(change).toHaveBeenLastCalledWith('bypass', 1);
  view.rerender(
    <MixRelayProvider>
      <AudioNodeCard {...card} parameterControls={[...controls, { ...bypass, value: 1 }]} />
    </MixRelayProvider>,
  );
  const pressed = screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') });
  expect(pressed).toHaveAttribute('aria-pressed', 'true');
  fireEvent.click(pressed);
  expect(change).toHaveBeenLastCalledWith('bypass', 0);
  view.rerender(
    <MixRelayProvider>
      <AudioNodeCard
        {...card}
        poweredOff
        parameterControls={[...controls, { ...bypass, value: 1 }]}
      />
    </MixRelayProvider>,
  );
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Bypassed') }),
  ).toBeDisabled();
});
