import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { CheckboxField } from '../../ui/CheckboxField';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/CheckboxField',
  component: CheckboxField,
  argTypes: {
    onChange: { control: false },
  },
  args: { label: '입력 사용', checked: false, disabled: false, onChange: fn() },
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
        <CheckboxField
          {...local}
          onChange={(checked) => {
            local.onChange(checked);
            set({ checked });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof CheckboxField>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Checked: Story = { args: { checked: true } };
export const Disabled: Story = { args: { checked: true, disabled: true } };

export const Keyboard: Story = {
  play: async ({ canvasElement }) => {
    const input = within(canvasElement).getByRole('checkbox', { name: '입력 사용' });
    await userEvent.tab();
    await expect(input).toHaveFocus();
    await userEvent.keyboard(' ');
    await waitFor(() => expect(input).toBeChecked());
    await userEvent.tab();
    await waitFor(() => expect(input).toBeChecked());
  },
};
