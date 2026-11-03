import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { NativeSampleFormatField } from '../../ui/NativeSampleFormatField';
import { sampleFormats } from './fixtures';

const meta = {
  title: 'Core/Playgrounds/NativeSampleFormatField',
  component: NativeSampleFormatField,
  argTypes: {
    onChange: { control: false },
  },
  args: {
    formats: sampleFormats,
    render: true,
    capture: true,
    label: 'PCM 형식',
    unknownLabel: '미확인',
    mixedLabel: '방향별 형식 불일치',
    disabled: false,
    error: '',
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
          '변경 요청과 확인된 formats를 구별합니다. 선택 callback은 readback을 자동 변경하지 않습니다.',
      },
    },
  },
} satisfies Meta<typeof NativeSampleFormatField>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const Mixed: Story = {
  args: {
    formats: {
      ...sampleFormats,
      capture: {
        ...sampleFormats.capture,
        current: { sampleRate: 48000, channels: 2, sampleFormat: 'signed24' },
      },
    },
  },
};
export const Unavailable: Story = {
  args: {
    formats: {
      render: { current: null, supported: [] },
      capture: { current: null, supported: [] },
    },
  },
};
export const Failed: Story = { args: { error: '형식을 적용하지 못했습니다.' } };
