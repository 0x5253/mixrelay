import type { Meta, StoryObj } from '@storybook/react-vite';
import { ActionButton } from '../../ui/ActionButton';
import { FormActions } from '../../ui/FormActions';

const meta = {
  title: 'Core/Playgrounds/FormActions',
  component: FormActions,
  argTypes: {
    align: { control: 'select', options: ['start', 'end'] },
    children: { control: false },
  },
  args: {
    align: 'end',
    children: (
      <>
        <ActionButton>취소</ActionButton>
        <ActionButton emphasis="strong">확인</ActionButton>
      </>
    ),
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
} satisfies Meta<typeof FormActions>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Start: Story = { args: { align: 'start' } };
export const LongLabels: Story = {
  args: {
    children: (
      <>
        <ActionButton>기존 작업을 보존하고 취소</ActionButton>
        <ActionButton>확인한 설정 적용</ActionButton>
      </>
    ),
  },
};
