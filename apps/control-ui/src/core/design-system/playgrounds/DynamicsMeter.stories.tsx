import type { Meta, StoryObj } from '@storybook/react-vite';
import { AudioMeterProvider } from '../../audio-meter/AudioMeterProvider';
import { DynamicsMeter } from '../../ui/DynamicsMeter';
import { dynamicsContract, sampleMeterStore, thresholdControls } from './fixtures';

const meta = {
  title: 'Core/Playgrounds/DynamicsMeter',
  component: DynamicsMeter,
  argTypes: {},
  args: {
    contract: dynamicsContract,
    controls: thresholdControls,
    disabled: false,
    bypassed: false,
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
          '계측은 결정적 샘플 store입니다. 미수신 상태는 기존 Waiting 사용례에서 비교합니다.',
      },
    },
  },
  render: (args) => (
    <div style={{ width: 420, maxWidth: '100%' }}>
      <AudioMeterProvider store={sampleMeterStore}>
        <DynamicsMeter {...args} />
      </AudioMeterProvider>
    </div>
  ),
} satisfies Meta<typeof DynamicsMeter>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Bypassed: Story = { args: { bypassed: true } };
export const Disabled: Story = { args: { disabled: true } };
