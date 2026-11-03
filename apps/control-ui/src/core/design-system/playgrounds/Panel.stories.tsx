import type { Meta, StoryObj } from '@storybook/react-vite';
import { ActionButton } from '../../ui/ActionButton';
import { Panel } from '../../ui/Panel';
import { Text } from '../../ui/Text';

const meta = {
  title: 'Core/Playgrounds/Panel',
  component: Panel,
  argTypes: {
    children: { control: false },
    trailing: { control: false },
  },
  args: {
    title: '장치 설정',
    children: <Text>장치에서 확인한 값을 표시합니다.</Text>,
    trailing: <ActionButton>확인</ActionButton>,
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
} satisfies Meta<typeof Panel>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const WithoutAction: Story = { args: { trailing: undefined } };
export const LongTitle: Story = { args: { title: '입력과 출력 장치의 형식과 버퍼 설정' } };
