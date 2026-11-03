import type { Meta, StoryObj } from '@storybook/react-vite';
import { InfoPopover } from '../../ui/InfoPopover';

import { Text } from '../../ui/Text';

const meta = {
  title: 'Core/Playgrounds/InfoPopover',
  component: InfoPopover,
  argTypes: {
    children: { control: false },
  },
  args: {
    label: '입출력 형식 정보',
    critical: false,
    children: <Text>48 kHz · 2 CH · 실제 확인 값 표본</Text>,
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
} satisfies Meta<typeof InfoPopover>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Critical: Story = { args: { critical: true } };
export const LongContent: Story = {
  args: {
    children: (
      <Text>확인된 장치 값과 변경 요청은 별개입니다. 현재 지원 범위 안에서 설정을 선택합니다.</Text>
    ),
  },
};
