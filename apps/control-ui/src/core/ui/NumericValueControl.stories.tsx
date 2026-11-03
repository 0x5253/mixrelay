import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { NumericValueControl } from './NumericValueControl';

function Demo({
  disabled = false,
  frequency = false,
  compact = false,
}: {
  disabled?: boolean;
  frequency?: boolean;
  compact?: boolean;
}) {
  const [value, setValue] = useState(frequency ? 150 : 0);
  return (
    <NumericValueControl
      label={frequency ? 'Frequency' : 'Gain'}
      value={value}
      minimum={frequency ? 20 : -24}
      maximum={frequency ? 20000 : 24}
      step={frequency ? 1 : 0.1}
      scale={frequency ? 'logarithmic' : 'linear'}
      suffix={frequency ? 'Hz' : 'dB'}
      disabled={disabled}
      density={compact ? 'compact' : 'regular'}
      onChange={setValue}
    />
  );
}
const meta = { title: 'Core/Numeric Value Control', component: Demo } satisfies Meta<typeof Demo>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Gain: Story = {};
export const Frequency: Story = { args: { frequency: true } };
export const Disabled: Story = { args: { disabled: true } };
export const Compact: Story = { args: { compact: true } };
