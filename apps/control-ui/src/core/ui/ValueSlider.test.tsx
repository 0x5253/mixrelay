import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { ValueSlider } from './ValueSlider';

it('exposes native range limits and units while waiting for confirmed readback', () => {
  const change = vi.fn();
  const view = render(
    <MixRelayProvider>
      <ValueSlider
        label="Input gain"
        value={-6}
        min={-60}
        max={12}
        step={0.5}
        valueText="-6 dB"
        onChange={change}
      />
    </MixRelayProvider>,
  );
  const slider = screen.getByRole('slider', { name: 'Input gain' });
  expect(slider).toHaveValue('-6');
  expect(slider).toHaveAttribute('min', '-60');
  expect(slider).toHaveAttribute('max', '12');
  expect(slider).toHaveAttribute('step', '0.5');
  expect(slider).toHaveAttribute('aria-valuetext', '-6 dB');
  expect(slider).toHaveAccessibleDescription('');
  fireEvent.change(slider, { target: { value: '-5.5' } });
  expect(change).toHaveBeenCalledExactlyOnceWith(-5.5);
  expect(slider).toHaveValue('-6');
  view.rerender(
    <MixRelayProvider>
      <ValueSlider
        label="Input gain"
        value={-5.5}
        min={-60}
        max={12}
        step={0.5}
        valueText="-5.5 dB"
        onChange={change}
      />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('slider')).toBe(slider);
  expect(slider).toHaveValue('-5.5');
  expect(slider).toHaveAttribute('aria-valuetext', '-5.5 dB');
});

it('exposes a disabled native range and defaults to whole steps', () => {
  render(
    <MixRelayProvider>
      <ValueSlider label="Threshold" value={20} min={0} max={100} disabled onChange={vi.fn()} />
    </MixRelayProvider>,
  );
  const slider = screen.getByRole('slider', { name: 'Threshold' });
  expect(slider).toBeDisabled();
  expect(slider).toHaveAttribute('step', '1');
  expect(slider).toHaveAccessibleDescription('');
});

it('keeps logarithmic keyboard behavior in actual units and ignores non-adjustment and disabled keys', () => {
  const change = vi.fn();
  const props = {
    label: 'Frequency',
    value: 1000,
    min: 20,
    max: 20000,
    step: 1,
    scale: 'logarithmic' as const,
    onChange: change,
  };
  const view = render(
    <MixRelayProvider>
      <ValueSlider {...props} />
    </MixRelayProvider>,
  );
  const slider = screen.getByRole('slider');
  for (const [key, value] of [
    ['ArrowUp', 1001],
    ['ArrowDown', 999],
    ['ArrowLeft', 999],
    ['ArrowRight', 1001],
    ['PageUp', 1010],
    ['PageDown', 990],
    ['Home', 20],
    ['End', 20000],
  ] as const) {
    fireEvent.keyDown(slider, { key });
    expect(change).toHaveBeenLastCalledWith(value);
  }
  fireEvent.keyDown(slider, { key: 'Tab' });
  expect(change).toHaveBeenCalledTimes(8);
  view.rerender(
    <MixRelayProvider>
      <ValueSlider {...props} disabled />
    </MixRelayProvider>,
  );
  fireEvent.keyDown(slider, { key: 'ArrowUp' });
  expect(change).toHaveBeenCalledTimes(8);
});
