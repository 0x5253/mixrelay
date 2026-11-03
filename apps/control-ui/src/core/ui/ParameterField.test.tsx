import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { ParameterField } from './ParameterField';

const frequency = {
  id: 'frequency',
  label: 'Frequency',
  minimum: 20,
  maximum: 20000,
  step: 1,
  suffix: 'Hz',
  value: 1000,
  scale: 'logarithmic' as const,
};
it('accepts precise frequency entry and exposes the unit on both controls', () => {
  const change = vi.fn();
  render(
    <MixRelayProvider>
      <ParameterField control={frequency} onChange={change} />
    </MixRelayProvider>,
  );
  const entry = screen.getByRole('textbox', { name: 'Frequency' });
  expect(entry).toHaveAccessibleDescription('Hz');
  fireEvent.focus(entry);
  fireEvent.change(entry, { target: { value: '2345' } });
  fireEvent.blur(entry);
  expect(change).toHaveBeenCalledWith(2345);
  const slider = screen.getByRole('slider', { name: 'Frequency' });
  expect(slider).toHaveAttribute('aria-valuetext', '1000 Hz');
  expect(slider).toHaveAttribute('aria-valuemin', '20');
  expect(slider).toHaveAttribute('aria-valuemax', '20000');
  expect(slider).toHaveAttribute('aria-valuenow', '1000');
  fireEvent.change(slider, { target: { value: String(Number(slider.getAttribute('max')) / 2) } });
  expect(change).toHaveBeenLastCalledWith(632);
  fireEvent.keyDown(slider, { key: 'ArrowUp' });
  expect(change).toHaveBeenLastCalledWith(1001);
});
it('keeps disabled numeric and slider controls out of editing', () => {
  render(
    <MixRelayProvider>
      <ParameterField control={frequency} disabled onChange={vi.fn()} />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('textbox')).toBeDisabled();
  expect(screen.getByRole('slider')).toBeDisabled();
});
it('keeps unitless scalar entry valid after clearing it without submitting an invalid value', () => {
  const change = vi.fn();
  render(
    <MixRelayProvider>
      <ParameterField
        control={{
          id: 'ratio',
          label: 'Ratio',
          minimum: 1,
          maximum: 20,
          step: 0.1,
          suffix: '',
          value: 4,
        }}
        onChange={change}
      />
    </MixRelayProvider>,
  );
  const entry = screen.getByRole('textbox', { name: 'Ratio' });
  expect(entry).not.toHaveAttribute('aria-describedby');
  expect(entry).toHaveAccessibleDescription('');
  fireEvent.focus(entry);
  fireEvent.change(entry, { target: { value: '' } });
  fireEvent.blur(entry);
  expect(change).not.toHaveBeenCalled();
  fireEvent.focus(entry);
  fireEvent.change(entry, { target: { value: '6.5' } });
  fireEvent.blur(entry);
  expect(change).toHaveBeenCalledExactlyOnceWith(6.5);
});
it('uses one named select for discrete choices without a duplicate slider', () => {
  const change = vi.fn();
  render(
    <MixRelayProvider>
      <ParameterField
        control={{
          ...frequency,
          label: 'Mode',
          value: 0,
          choices: [
            { value: 0, label: 'Unchanged' },
            { value: 1, label: 'Mono average' },
          ],
        }}
        onChange={change}
      />
    </MixRelayProvider>,
  );
  expect(screen.queryByRole('slider')).toBeNull();
  expect(screen.queryByRole('textbox')).toBeNull();
  expect(screen.getByRole('button', { name: /Mode/ })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: /Mode/ }));
  fireEvent.click(screen.getByRole('option', { name: 'Mono average' }));
  expect(change).toHaveBeenCalledExactlyOnceWith(1);
});

it('clearing numeric entry does not submit NaN, and valid entry still commits afterward', () => {
  const change = vi.fn();
  render(
    <MixRelayProvider>
      <ParameterField
        control={{ ...frequency, presentation: 'knob', suffix: '' }}
        onChange={change}
      />
    </MixRelayProvider>,
  );
  const entry = screen.getByRole('textbox', { name: 'Frequency' });
  expect(entry).toHaveAccessibleDescription('');
  fireEvent.focus(entry);
  fireEvent.change(entry, { target: { value: '' } });
  fireEvent.blur(entry);
  expect(change).not.toHaveBeenCalled();
  fireEvent.focus(entry);
  fireEvent.change(entry, { target: { value: '3456' } });
  fireEvent.blur(entry);
  expect(change).toHaveBeenCalledExactlyOnceWith(3456);
});
it('retains exact channel position entry and reports the left/right range on its knob', () => {
  const change = vi.fn();
  render(
    <MixRelayProvider>
      <ParameterField
        control={{
          id: 'position',
          label: 'Position',
          value: 0,
          minimum: -1,
          maximum: 1,
          step: 0.01,
          suffix: '',
          presentation: 'knob',
          endpoints: { minimum: 'L', maximum: 'R' },
        }}
        onChange={change}
      />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('slider')).toHaveAttribute('aria-valuetext', '0  (L -1 / R 1)');
  const entry = screen.getByRole('textbox', { name: 'Position' });
  fireEvent.focus(entry);
  fireEvent.change(entry, { target: { value: '-0.25' } });
  fireEvent.blur(entry);
  expect(change).toHaveBeenCalledExactlyOnceWith(-0.25);
});
