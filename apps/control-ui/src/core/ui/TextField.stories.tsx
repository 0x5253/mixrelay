import type { Meta, StoryObj } from '@storybook/react-vite';
import { useArgs, useEffect, useState } from 'storybook/preview-api';
import { expect, userEvent, waitFor, within } from 'storybook/test';
import { TextField } from './TextField';

const meta = {
  title: 'Core/Controls/TextField',
  component: TextField,
  args: {
    label: '그래프 이름',
    value: 'Studio',
    description: '저장할 그래프 이름을 입력합니다.',
    error: '',
    type: 'text',
    disabled: false,
    readOnly: false,
    required: false,
  },
  argTypes: {
    label: { control: 'text' },
    value: {
      control: 'text',
      description:
        '입력은 로컬 상태가 소유하고 blur에서 args에 반영합니다. Controls 변경은 즉시 입력에 반영합니다.',
    },
    description: { control: 'text' },
    error: { control: 'text', description: '실패 이유와 입력의 invalid 관계.' },
    type: { control: 'select', options: ['text', 'password', 'email', 'search'] },
    disabled: { control: 'boolean' },
    readOnly: { control: 'boolean' },
    required: { control: 'boolean' },
    onChange: {
      control: false,
      description: 'input 이벤트. 제품의 상태 소유자가 현재 값을 갱신합니다.',
    },
  },
  parameters: {
    controls: {
      include: [
        'label',
        'value',
        'description',
        'error',
        'type',
        'disabled',
        'readOnly',
        'required',
      ],
    },
    docs: { source: { type: 'dynamic' } },
  },
  render: function ControlledField(args) {
    const [, updateArgs] = useArgs();
    const [value, setValue] = useState(args.value);
    useEffect(() => setValue(args.value), [args.value]);
    return (
      <TextField
        {...args}
        value={value}
        onChange={(event) => setValue(event.currentTarget.value)}
        onBlur={() => updateArgs({ value })}
      />
    );
  },
} satisfies Meta<typeof TextField>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Playground: Story = {};
export const Invalid: Story = { args: { error: '이름을 저장하지 못했습니다. 입력은 보존됩니다.' } };
export const ReadOnly: Story = { args: { readOnly: true } };
export const Disabled: Story = { args: { disabled: true } };
export const Password: Story = {
  args: { label: '샘플 키', value: 'example-only', type: 'password' },
};
export const Editing: Story = {
  play: async ({ canvasElement, args }) => {
    const input = within(canvasElement).getByRole('textbox', { name: String(args.label) });
    await userEvent.clear(input);
    await userEvent.type(input, 'Live Studio');
    await waitFor(() => expect(input).toHaveValue('Live Studio'));
    await userEvent.tab();
    await waitFor(() => expect(input).toHaveValue('Live Studio'));
  },
};
