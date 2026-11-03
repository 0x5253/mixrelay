import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { AudioNodeEndpointOptions } from '../../ui/AudioNodeEndpointOptions';
import { endpointControl } from './fixtures';

const meta = {
  title: 'Core/Playgrounds/AudioNodeEndpointOptions',
  component: AudioNodeEndpointOptions,
  argTypes: { onChange: { control: false } },
  args: { control: endpointControl, onChange: fn() },
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
          '이름 → 채널 → 샘플레이트 → 비트 심도 순서로 편집합니다. 변경은 요청만 전달하며 표시 값은 control의 readback이 결정합니다.',
      },
    },
  },
} satisfies Meta<typeof AudioNodeEndpointOptions>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const ReadOnlyName: Story = { args: { control: { ...endpointControl, canRename: false } } };
export const RenderOnly: Story = {
  args: { control: { ...endpointControl, captureChannels: 0, allowedCaptureChannels: [] } },
};
export const SingleChoice: Story = {
  args: {
    control: {
      ...endpointControl,
      allowedRenderChannels: [2],
      allowedCaptureChannels: [2],
      sampleRateOptions: [48000],
    },
  },
};
export const Failed: Story = {
  args: {
    control: {
      ...endpointControl,
      rateError: '샘플레이트를 적용하지 못했습니다.',
      formatError: '비트 심도를 적용하지 못했습니다.',
    },
  },
};
