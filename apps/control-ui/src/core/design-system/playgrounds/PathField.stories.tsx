import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { PathField } from '../../ui/PathField';

const meta = {
  title: 'Core/Playgrounds/PathField',
  component: PathField,
  argTypes: {
    onChoose: { control: false },
  },
  args: {
    label: '저장 위치',
    value: '/Users/example/Recordings',
    actionLabel: '폴더 선택',
    disabled: false,
    onChoose: fn(),
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
        component: '경로는 읽기 전용이며 폴더 선택 callback은 Actions에만 기록합니다.',
      },
    },
  },
} satisfies Meta<typeof PathField>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const WithoutChooser: Story = { args: { onChoose: undefined } };
export const LongPath: Story = {
  args: { value: '/Users/example/Recordings/Studio/2026/긴 이름을 가진 녹음 폴더' },
};
