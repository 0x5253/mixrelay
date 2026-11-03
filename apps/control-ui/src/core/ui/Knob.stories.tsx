import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import nodeStyles from './AudioNodeCard.module.css';
import { ParameterField } from './ParameterField';

function Demo({
  disabled = false,
  logarithmic = false,
}: {
  disabled?: boolean;
  logarithmic?: boolean;
}) {
  const [value, setValue] = useState(logarithmic ? 1000 : 0);
  return (
    <div
      className={nodeStyles.knobGroup}
      data-columns={1}
      style={{ width: 'var(--space-24)', maxWidth: '100%' }}
    >
      <ParameterField
        control={{
          id: 'demo',
          label: logarithmic ? 'Frequency' : 'Gain',
          value,
          minimum: logarithmic ? 20 : -24,
          maximum: logarithmic ? 20000 : 24,
          step: logarithmic ? 1 : 0.1,
          suffix: logarithmic ? 'Hz' : 'dB',
          scale: logarithmic ? 'logarithmic' : 'linear',
          presentation: 'knob',
        }}
        disabled={disabled}
        onChange={setValue}
      />
    </div>
  );
}
const meta = { title: 'Core/Knob', component: Demo } satisfies Meta<typeof Demo>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Gain: Story = {};
export const Frequency: Story = { args: { logarithmic: true } };
export const Disabled: Story = { args: { disabled: true } };
