import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { ValueSlider } from '../../ui/ValueSlider';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/ValueSlider',
  component: ValueSlider,
  argTypes: {
    scale: { control: 'select', options: ['linear', 'logarithmic'] },
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
        <ValueSlider
          {...local}
          onChange={(value) => {
            local.onChange(value);
            set({ value });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof ValueSlider>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const Logarithmic: Story = {
  args: { label: '주파수', value: 1000, min: 20, max: 20000, scale: 'logarithmic' },
};
