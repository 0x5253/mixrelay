import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'storybook/preview-api';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { i18n } from '../../i18n';
import { ActionButton } from '../../ui/ActionButton';
import { NameDialog } from '../../ui/NameDialog';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/NameDialog',
  component: NameDialog,
  argTypes: {
    onCancel: { control: false },
    onCreate: { control: false },
  },
  args: {
    title: '새 그래프',
    initialName: 'Studio',
    nameLabel: '그래프 이름',
    createLabel: '만들기',
    enabled: true,
    error: '',
    onCancel: fn(),
    onCreate: fn(),
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
          'initialName은 최초 mount의 초기값입니다. Controls에서 바꾼 뒤 닫고 다시 열면 새 초기값을 사용합니다.',
      },
    },
  },
  render: function Interactive(args) {
    const { local, commit } = usePlaygroundState(args);
    const [visible, setVisible] = useState(true);
    return (
      <fieldset
        aria-label="Playground 예제"
        onBlur={commit}
        style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
      >
        {visible ? (
          <NameDialog
            {...local}
            onCancel={() => {
              local.onCancel();
              setVisible(false);
            }}
            onCreate={(name) => {
              local.onCreate(name);
              setVisible(false);
            }}
          />
        ) : (
          <ActionButton onClick={() => setVisible(true)}>이름 입력 열기</ActionButton>
        )}
      </fieldset>
    );
  },
} satisfies Meta<typeof NameDialog>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { enabled: false } };
export const Failed: Story = { args: { error: '생성하지 못했습니다. 입력을 유지합니다.' } };

export const EmptyReset: Story = {
  args: {
    title: '표시 이름 변경',
    nameLabel: '표시 이름',
    createLabel: '저장',
    initialName: '',
    allowEmpty: true,
    maxLength: 128,
    placeholder: 'Firefox',
  },
};

export const FooterKeyboard: Story = {
  play: async ({ canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    const input = await body.findByRole('textbox', { name: '그래프 이름' });
    await waitFor(() => expect(input).toHaveFocus());
    await userEvent.tab();
    const cancel = body.getByRole('button', { name: i18n.t('common.cancel') });
    await expect(cancel).toHaveFocus();
    await userEvent.tab();
    await expect(body.getByRole('button', { name: '만들기' })).toHaveFocus();
    await userEvent.tab({ shift: true });
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(body.queryByRole('dialog')).not.toBeInTheDocument());
    await userEvent.click(within(canvasElement).getByRole('button', { name: '이름 입력 열기' }));
    await waitFor(() => expect(body.getByRole('textbox', { name: '그래프 이름' })).toHaveFocus());
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(body.queryByRole('dialog')).not.toBeInTheDocument());
  },
};
