import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { ActionButton } from '../../ui/ActionButton';
import { ConfirmationDialog } from '../../ui/ConfirmationDialog';

import { Text } from '../../ui/Text';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/ConfirmationDialog',
  component: ConfirmationDialog,
  argTypes: {
    children: { control: false },
    onCancel: { control: false },
    onConfirm: { control: false },
  },
  args: {
    title: '그래프 삭제',
    opened: false,
    busy: false,
    disabled: false,
    cancelLabel: '취소',
    children: <Text>Studio 그래프 삭제 확인 표본입니다.</Text>,
    onCancel: fn(),
    onConfirm: fn(),
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
        <ActionButton onClick={() => set({ opened: true })}>삭제 확인 열기</ActionButton>
        <ConfirmationDialog
          {...local}
          onCancel={() => {
            local.onCancel();
            set({ opened: false });
          }}
          onConfirm={() => {
            local.onConfirm();
            set({ opened: false });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof ConfirmationDialog>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Busy: Story = { args: { opened: true, busy: true, disabled: true } };
export const Disabled: Story = { args: { opened: true, disabled: true } };
