import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { NumericValueControl } from '../../ui/NumericValueControl';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/NumericValueControl',
  component: NumericValueControl,
  argTypes: {
    scale: { control: 'select', options: ['linear', 'logarithmic'] },
    density: { control: 'select', options: ['regular', 'compact'] },
    onChange: { control: false },
  },
  args: {
    label: '게인',
    value: 0,
    minimum: -60,
    maximum: 12,
    step: 0.5,
    suffix: 'dB',
    density: 'regular',
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
        <NumericValueControl
          {...local}
          onChange={(value) => {
            local.onChange(value);
            set({ value });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof NumericValueControl>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Compact: Story = { args: { density: 'compact' } };
export const Disabled: Story = { args: { disabled: true } };

export const Editing: Story = {
  play: async ({ canvasElement }) => {
    const input = within(canvasElement).getByRole('textbox', { name: '게인' });
    await userEvent.clear(input);
    await userEvent.type(input, '-12.5');
    await userEvent.tab();
    await waitFor(() => expect(input).toHaveValue('-12.5'));
    await userEvent.click(input);
    await userEvent.keyboard('{ArrowUp}');
    await waitFor(() => expect(input).toHaveValue('-12'));
  },
};
