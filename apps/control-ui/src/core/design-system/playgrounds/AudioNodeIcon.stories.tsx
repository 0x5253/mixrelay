import type { Meta, StoryObj } from '@storybook/react-vite';
import { AudioNodeIcon } from '../../ui/AudioNodeIcon';

const meta = {
  title: 'Core/Playgrounds/AudioNodeIcon',
  component: AudioNodeIcon,
  argTypes: {
    icon: { control: 'inline-radio', options: ['APP', 'GAIN', 'MIX', 'ADC', 'MIC', 'DAC', 'IO'] },
  },
  args: { icon: 'APP' },
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
          '발견된 앱·플랫폼 아이콘(iconUrl)이 우선이며, 없거나 불러오지 못한 경우에만 icon 종류의 공통 기호를 표시합니다.',
      },
    },
  },
} satisfies Meta<typeof AudioNodeIcon>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const ProvidedIcon: Story = {
  args: {
    icon: 'APP',
    iconUrl:
      'data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 16 16%22%3E%3Crect width=%2216%22 height=%2216%22 rx=%224%22 fill=%22%23235f96%22/%3E%3C/svg%3E',
  },
};
export const FailedIcon: Story = { args: { icon: 'APP', iconUrl: 'data:image/png;base64,broken' } };
export const Input: Story = { args: { icon: 'ADC' } };
export const Output: Story = { args: { icon: 'DAC' } };
export const Processor: Story = { args: { icon: 'GAIN' } };
export const Device: Story = { args: { icon: 'IO' } };
