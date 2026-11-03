import type { Meta, StoryObj } from '@storybook/react-vite';
import { ActionButton } from '../../ui/ActionButton';
import { ApplicationStatusBar } from '../../ui/ApplicationStatusBar';
import { Text } from '../../ui/Text';

const meta = {
  title: 'Core/Playgrounds/ApplicationStatusBar',
  component: ApplicationStatusBar,
  argTypes: {
    information: { control: false },
    controls: { control: false },
  },
  args: {
    information: <Text kind="supporting">0.1.3 · 무료</Text>,
    controls: <ActionButton>오디오 일시정지</ActionButton>,
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
} satisfies Meta<typeof ApplicationStatusBar>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const WithoutInformation: Story = { args: { information: undefined } };
export const LongInformation: Story = {
  args: { information: <Text>0.1.3 · 인증 상태를 확인하지 못했습니다.</Text> },
};

export const WithDiagnostics: Story = {
  args: {
    controls: (
      <>
        <ActionButton density="compact">오디오 일시정지</ActionButton>
        <ActionButton density="compact">진단 · 19</ActionButton>
      </>
    ),
  },
};
