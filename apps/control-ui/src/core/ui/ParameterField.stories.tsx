import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import nodeStyles from './AudioNodeCard.module.css';
import { ParameterField, type ParameterFieldControl } from './ParameterField';

function Demo(props: { control: ParameterFieldControl; disabled?: boolean }) {
  const [value, setValue] = useState(props.control.value);
  return (
    <div
      className={props.control.presentation === 'knob' ? nodeStyles.knobGroup : undefined}
      data-columns={props.control.presentation === 'knob' ? 1 : undefined}
      style={{ width: 'var(--node-default-width)', maxWidth: '100%' }}
    >
      <ParameterField
        control={{ ...props.control, value }}
        disabled={props.disabled}
        onChange={setValue}
      />
    </div>
  );
}
const meta = {
  title: 'Core/Parameter Field',
  component: Demo,
  args: {
    control: {
      id: 'frequency',
      label: 'Frequency',
      minimum: 20,
      maximum: 20000,
      step: 1,
      suffix: 'Hz',
      value: 1000,
      scale: 'logarithmic',
    },
  },
} satisfies Meta<typeof Demo>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Frequency: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const FrequencyKnob: Story = {
  args: {
    control: { ...meta.args.control, presentation: 'knob' },
  },
};
export const DisabledKnob: Story = {
  args: {
    control: { ...meta.args.control, presentation: 'knob' },
    disabled: true,
  },
};
export const Fractional: Story = {
  args: {
    control: {
      id: 'mix',
      label: 'Mix',
      minimum: 0,
      maximum: 1,
      step: 0.01,
      suffix: '',
      value: 0.25,
    },
  },
};
export const Discrete: Story = {
  args: {
    control: {
      id: 'mode',
      label: 'Channel mode',
      minimum: 0,
      maximum: 5,
      step: 1,
      suffix: '',
      value: 1,
      choices: [
        { value: 0, label: 'Preserve stereo', description: 'L → L, R → R' },
        { value: 1, label: 'Mono sum', description: '(L + R) / 2 → L/R' },
        { value: 4, label: 'Swap L/R', description: 'L ↔ R' },
      ],
    },
  },
};
export const LongLabel: Story = {
  args: {
    control: {
      ...meta.args.control,
      label: '[!! Frequency control with an expanded translated label !!]',
    },
  },
};
