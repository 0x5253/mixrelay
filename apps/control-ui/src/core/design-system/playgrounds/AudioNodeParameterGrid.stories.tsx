import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { AudioNodeParameterGrid } from '../../ui/AudioNodeParameterGrid';
import { knobParameters, mixedParameters } from './fixtures';

const meta = {
  title: 'Core/Playgrounds/AudioNodeParameterGrid',
  component: AudioNodeParameterGrid,
  argTypes: { onChange: { control: false } },
  args: { controls: knobParameters, disabled: false, onChange: fn() },
  decorators: [
    (Story) => (
      <div style={{ width: 208 }}>
        <Story />
      </div>
    ),
  ],
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
          '연속된 노브는 최대 3열로 묶고, 선택형·정밀 입력·슬라이더는 한 행씩 배치합니다. 값은 Feature가 확인한 readback을 따릅니다.',
      },
    },
  },
} satisfies Meta<typeof AudioNodeParameterGrid>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Mixed: Story = { args: { controls: mixedParameters } };
export const TwoKnobs: Story = { args: { controls: knobParameters.slice(0, 2) } };
export const Disabled: Story = { args: { disabled: true } };
