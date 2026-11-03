import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { ParameterField } from '../../ui/ParameterField';
import { gainControl } from './fixtures';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/ParameterField',
  component: ParameterField,
  argTypes: {
    onChange: { control: false },
  },
  args: { control: gainControl, disabled: false, onChange: fn() },
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
        <ParameterField
          {...local}
          onChange={(value) => {
            local.onChange(value);
            set({ control: { ...local.control, value } });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof ParameterField>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Knob: Story = { args: { control: { ...gainControl, presentation: 'knob' } } };
export const Choices: Story = {
  args: {
    control: {
      ...gainControl,
      value: 0,
      choices: [
        { value: 0, label: 'Stereo', description: 'L → L, R → R' },
        { value: 1, label: 'Mono', description: '(L + R) / 2' },
      ],
    },
  },
};
export const Disabled: Story = { args: { disabled: true } };
