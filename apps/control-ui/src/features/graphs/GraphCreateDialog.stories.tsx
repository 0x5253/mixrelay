import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { i18n } from '../../core/i18n';
import { ActionButton } from '../../core/ui/ActionButton';
import { GraphCreateDialog } from './GraphCreateDialog';

const meta = {
  title: 'Features/Graphs/Create',
  component: GraphCreateDialog,
  args: {
    suggestedName: 'Studio',
    busy: false,
    creating: false,
    error: null,
    onCancel: fn(),
    onCreate: fn(),
  },
  argTypes: { onCancel: { control: false }, onCreate: { control: false } },
  render: function Create(args) {
    const [opened, setOpened] = useState(false);
    return (
      <>
        <ActionButton onClick={() => setOpened(true)}>그래프 생성 열기</ActionButton>
        {opened ? (
          <GraphCreateDialog
            {...args}
            onCancel={() => {
              args.onCancel();
              setOpened(false);
            }}
            onCreate={args.onCreate}
          />
        ) : null}
      </>
    );
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(within(canvasElement).getByRole('button', { name: '그래프 생성 열기' }));
    const dialog = await within(canvasElement.ownerDocument.body).findByRole('dialog');
    await waitFor(() => expect(dialog).toBeVisible());
  },
} satisfies Meta<typeof GraphCreateDialog>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Editing: Story = {};
export const EmptyName: Story = { args: { suggestedName: '' } };
export const Creating: Story = { args: { busy: true, creating: true } };
export const Failed: Story = {
  args: { error: '그래프를 만들지 못했습니다. 입력한 이름은 유지합니다.' },
};
export const KeyboardSubmit: Story = {
  play: async ({ canvasElement, args }) => {
    await userEvent.click(within(canvasElement).getByRole('button', { name: '그래프 생성 열기' }));
    const dialog = await within(canvasElement.ownerDocument.body).findByRole('dialog');
    const input = within(dialog).getByRole('textbox');
    await userEvent.clear(input);
    await userEvent.type(input, '  Live Studio  ');
    await userEvent.keyboard('{Enter}');
    await expect(args.onCreate).toHaveBeenCalledWith('Live Studio');
    await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('common.cancel') }));
    await waitFor(() =>
      expect(within(canvasElement).getByRole('button', { name: '그래프 생성 열기' })).toHaveFocus(),
    );
  },
};
