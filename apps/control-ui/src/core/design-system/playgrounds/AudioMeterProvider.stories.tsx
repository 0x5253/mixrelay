import type { Meta, StoryObj } from '@storybook/react-vite';
import { AudioMeterProvider } from '../../audio-meter/AudioMeterProvider';
import { DynamicsMeter } from '../../ui/DynamicsMeter';
import { dynamicsContract, sampleMeterStore, thresholdControls } from './fixtures';

const meta = {
  title: 'Core/Playgrounds/AudioMeterProvider',
  component: AudioMeterProvider,
  argTypes: {
    store: { control: false },
    children: { control: false },
  },
  args: {
    store: sampleMeterStore,
    children: <DynamicsMeter contract={dynamicsContract} controls={thresholdControls} />,
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
          'store는 Controls에서 직렬화하지 않습니다. Unavailable은 null을 공급해 미수신을 표시합니다.',
      },
    },
  },
} satisfies Meta<typeof AudioMeterProvider>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Unavailable: Story = { args: { store: null } };
