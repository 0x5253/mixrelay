import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { GraphicEqualizer } from '../../ui/GraphicEqualizer';
import { equalizerContract, equalizerControls } from './fixtures';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/GraphicEqualizer',
  component: GraphicEqualizer,
  argTypes: {
    onChange: { control: false },
    onParametersChange: { control: false },
  },
  args: {
    contract: equalizerContract,
    controls: equalizerControls,
    disabled: false,
    bypassed: false,
    onChange: fn(),
    onParametersChange: fn(),
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
        <div style={{ width: 420, maxWidth: '100%' }}>
          <GraphicEqualizer
            {...local}
            onChange={(id, value) => {
              local.onChange(id, value);
              set({
                controls: local.controls.map((control) =>
                  control.id === id ? { ...control, value } : control,
                ),
              });
            }}
            onParametersChange={(updates) => {
              local.onParametersChange?.(updates);
              set({
                controls: local.controls.map((control) => ({
                  ...control,
                  value: updates.find((update) => update.id === control.id)?.value ?? control.value,
                })),
              });
            }}
          />
        </div>
      </fieldset>
    );
  },
} satisfies Meta<typeof GraphicEqualizer>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const Bypassed: Story = { args: { bypassed: true } };
