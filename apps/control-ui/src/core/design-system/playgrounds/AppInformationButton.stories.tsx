import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { AppInformationButton } from '../../ui/AppInformationButton';
import { Dialog } from '../../ui/Dialog';
import { Text } from '../../ui/Text';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/AppInformationButton',
  component: AppInformationButton,
  argTypes: {
    onOpen: { control: false },
  },
  args: { label: '제품 정보 열기', version: '0.1.3', opened: false, onOpen: fn() },
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
        <AppInformationButton
          {...local}
          onOpen={() => {
            local.onOpen();
            set({ opened: true });
          }}
        />
        <Dialog title="샘플 제품 정보" opened={local.opened} onClose={() => set({ opened: false })}>
          <Text>제품 정보 표본입니다.</Text>
        </Dialog>
      </fieldset>
    );
  },
} satisfies Meta<typeof AppInformationButton>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Opened: Story = { args: { opened: true } };
