import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard, type AudioNodeCardProps } from './AudioNodeCard';

it('uses disabled Core controls when a processing node is Off', () => {
  render(
    <MixRelayProvider>
      <AudioNodeCard
        title="Equalizer"
        icon="FX"
        category="Processor"
        inputCount={2}
        outputCount={2}
        poweredOff
        faceMode="compact"
        onParameterChange={vi.fn()}
        parameterControls={[
          {
            id: 'gain',
            label: 'Gain',
            value: 0,
            minimum: -24,
            maximum: 24,
            step: 0.1,
            suffix: 'dB',
            precise: true,
            presentation: 'knob',
          },
          {
            id: 'mode',
            label: 'Mode',
            value: 0,
            minimum: 0,
            maximum: 1,
            step: 1,
            suffix: '',
            choices: [
              { value: 0, label: 'Processing' },
              { value: 1, label: 'Bypassed' },
            ],
          },
        ]}
      />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('slider', { name: 'Gain' })).toBeDisabled();
  expect(screen.getByRole('textbox', { name: 'Gain' })).toBeDisabled();
  expect(screen.getByRole('button', { name: /Mode/ })).toBeDisabled();
});

it('keeps grouped knobs in the same input order when translated group names and values update', () => {
  const change = vi.fn();
  const controls: NonNullable<AudioNodeCardProps['parameterControls']> = [
    {
      id: 'frequency',
      label: 'Frequency',
      value: 1000,
      minimum: 20,
      maximum: 20000,
      step: 1,
      suffix: 'Hz',
      precise: true,
      presentation: 'knob',
      group: { id: 'mid-band', label: 'Mid' },
    },
    {
      id: 'q',
      label: 'Q',
      value: Number(Math.SQRT1_2.toFixed(3)),
      minimum: 0.1,
      maximum: 10,
      step: 0.001,
      suffix: '',
      precise: true,
      presentation: 'knob',
      group: { id: 'mid-band', label: 'Mid' },
    },
  ];
  const props = {
    title: 'Equalizer',
    icon: 'FX',
    category: 'Processor',
    inputCount: 2,
    outputCount: 2,
    faceMode: 'compact' as const,
    onParameterChange: change,
  };
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard {...props} parameterControls={controls} />
    </MixRelayProvider>,
  );
  expect(screen.getByText('Mid')).toBeInTheDocument();
  const frequency = screen.getByRole('slider', { name: 'Frequency' });
  const q = screen.getByRole('slider', { name: 'Q' });
  expect(frequency.compareDocumentPosition(q) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  view.rerender(
    <MixRelayProvider>
      <AudioNodeCard
        {...props}
        parameterControls={controls.map((control) => ({
          ...control,
          group: { id: 'mid-band', label: '[!! Expanded mid frequency band !!]' },
        }))}
      />
    </MixRelayProvider>,
  );
  expect(screen.queryByText('Mid')).toBeNull();
  expect(screen.getByText('[!! Expanded mid frequency band !!]')).toBeInTheDocument();
  expect(screen.getByRole('slider', { name: 'Frequency' })).toBe(frequency);
});
