import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { fn } from 'storybook/test';
import { Knob, type KnobProps } from '../../ui/Knob';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/Knob',
  component: Knob,
  argTypes: {
    step: { table: { defaultValue: { summary: '1' } } },
    scale: {
      control: 'select',
      options: ['linear', 'logarithmic'],
      table: { defaultValue: { summary: "'linear'" } },
    },
    onChange: { control: false },
  },
  args: {
    label: '게인',
    value: 0,
    min: -60,
    max: 12,
    step: 1,
    scale: 'linear',
    disabled: false,
    onChange: fn(),
  },
  parameters: {
    controls: {
      exclude: [
        'className',
        'classNames',
        'style',
        'styles',
        'vars',
        'unstyled',
        'color',
        'c',
        'size',
        'radius',
        'variant',
        'component',
        'renderRoot',
        'dangerouslySetInnerHTML',
      ],
    },
    docs: {
      source: { type: 'dynamic' },
      description: {
        component:
          'Controls는 실제 공개 props를 전달합니다. 샘플 callback은 Actions에 기록하며 제품 요청을 실행하지 않습니다.',
      },
    },
  },
  render: function Interactive(args) {
    const { local, set, commit } = usePlaygroundState(args);
    return (
      <fieldset
        aria-label="Playground 예제"
        onBlur={commit}
        style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
      >
        <Knob
          {...local}
          onChange={(value) => {
            local.onChange(value);
            set({ value });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof Knob>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const Logarithmic: Story = {
  args: { label: '주파수', value: 1000, min: 20, max: 20000, scale: 'logarithmic' },
};

export const Minimum: Story = { args: { min: 0, max: 100, value: 0 } };
export const Maximum: Story = { args: { min: 0, max: 100, value: 100 } };
export const BipolarNegative: Story = { args: { value: -30 } };
export const BipolarPositive: Story = { args: { value: 6 } };

function StateDial(props: KnobProps) {
  const [value, setValue] = useState(props.value);
  return (
    <div style={{ display: 'grid', justifyItems: 'center', gap: 'var(--space-1)' }}>
      <span>{props.label}</span>
      <Knob
        {...props}
        value={value}
        onChange={(value) => {
          props.onChange(value);
          setValue(value);
        }}
      />
    </div>
  );
}
export const States: Story = {
  render: (args) => (
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 'var(--space-6)' }}>
      {[
        { label: 'Minimum', min: 0, max: 100, value: 0 },
        { label: 'Half', min: 0, max: 100, value: 50 },
        { label: 'Maximum', min: 0, max: 100, value: 100 },
        { label: 'Negative', min: -60, max: 12, value: -30 },
        { label: 'Zero', min: -60, max: 12, value: 0 },
        { label: 'Positive', min: -60, max: 12, value: 6 },
        { label: 'Disabled min', min: 0, max: 100, value: 0, disabled: true },
        { label: 'Disabled max', min: 0, max: 100, value: 100, disabled: true },
        { label: 'Small', min: 0, max: 100, value: 1 },
        { label: 'Ratio', min: 1, max: 20, value: 4 },
        { label: 'Symmetric −', min: -24, max: 24, value: -6 },
        { label: 'Symmetric 0', min: -24, max: 24, value: 0 },
        { label: 'Symmetric +', min: -24, max: 24, value: 6 },
      ].map((state) => (
        <StateDial key={state.label} {...args} {...state} />
      ))}
    </div>
  ),
};
