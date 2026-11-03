import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { ActionButton } from '../../ui/ActionButton';
import { Text } from '../../ui/Text';
import { UpdatesDialog } from '../../ui/UpdatesDialog';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/UpdatesDialog',
  component: UpdatesDialog,
  argTypes: {
    children: { control: false },
    onClose: { control: false },
  },
  args: {
    title: '업데이트',
    opened: false,
    closeLabel: '닫기',
    children: <Text>설치 0.1.0 · 게시 0.1.1 · 실제 다운로드는 실행하지 않습니다.</Text>,
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
        <ActionButton onClick={() => set({ opened: true })}>업데이트 표본 열기</ActionButton>
        <UpdatesDialog
          {...local}
          onClose={() => {
            local.onClose();
            set({ opened: false });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof UpdatesDialog>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const LongContent: Story = {
  args: {
    opened: true,
    children: (
      <Text>
        MixRelay 전체 제품의 버전을 확인합니다. 설치 전 기존 설정과 작업을 보존합니다. 실제 업데이트
        여부는 UpdatesPort의 응답을 따릅니다.
      </Text>
    ),
  },
};
