import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { ActionButton } from '../../ui/ActionButton';
import { Dialog } from '../../ui/Dialog';
import { FormActions } from '../../ui/FormActions';

import { TextField } from '../../ui/TextField';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/Dialog',
  component: Dialog,
  argTypes: {
    size: { control: 'select', options: ['xs', 'sm', 'md'] },
    children: { control: false },
    onClose: { control: false },
  },
  args: {
    title: '그래프 설정',
    opened: false,
    busy: false,
    size: 'sm',
    withCloseButton: true,
    children: <TextField label="그래프 이름" defaultValue="Studio" />,
    onClose: fn(),
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
          'opened=false가 예제의 초기값입니다. 모달 열기로 열며 busy 표본은 Controls에서 busy를 해제한 뒤 닫습니다.',
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
        <ActionButton onClick={() => set({ opened: true })}>모달 열기</ActionButton>
        <Dialog
          {...local}
          onClose={() => {
            local.onClose();
            set({ opened: false });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof Dialog>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Busy: Story = { args: { opened: true, busy: true } };
export const Small: Story = { args: { opened: true, size: 'xs' } };
export const NoCloseButton: Story = { args: { opened: true, withCloseButton: false } };

export const CompactForm: Story = {
  args: { opened: true, size: 'xs', withCloseButton: false, title: '표시 이름 변경' },
  render: function Form(args) {
    const { local, set } = usePlaygroundState(args);
    const close = () => {
      local.onClose();
      set({ opened: false });
    };
    return (
      <>
        <ActionButton onClick={() => set({ opened: true })}>폼 열기</ActionButton>
        <Dialog {...local} onClose={close}>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              close();
            }}
          >
            <TextField data-autofocus label="표시 이름" defaultValue="회의용" />
            <FormActions>
              <ActionButton type="button" onClick={close}>
                취소
              </ActionButton>
              <ActionButton type="submit" emphasis="strong">
                저장
              </ActionButton>
            </FormActions>
          </form>
        </Dialog>
      </>
    );
  },
};
export const LongTitle: Story = {
  args: {
    opened: true,
    size: 'xs',
    title: '긴 그래프 인스턴스 이름의 표시 설정을 변경합니다 — LongUnbrokenDisplayNameForWrapping',
  },
  globals: { locale: 'pseudo' },
};

export const Keyboard: Story = {
  play: async ({ canvasElement }) => {
    const trigger = within(canvasElement).getByRole('button', { name: '모달 열기' });
    await userEvent.click(trigger);
    const dialog = await within(canvasElement.ownerDocument.body).findByRole('dialog');
    await waitFor(() => expect(dialog).toBeVisible());
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(trigger).toHaveFocus());
    await waitFor(() =>
      expect(
        within(canvasElement.ownerDocument.body).queryByRole('dialog'),
      ).not.toBeInTheDocument(),
    );
  },
};
