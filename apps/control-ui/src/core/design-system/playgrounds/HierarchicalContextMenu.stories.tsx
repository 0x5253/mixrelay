import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { HierarchicalContextMenu } from '../../ui/HierarchicalMenu';

const meta = {
  title: 'Core/Playgrounds/HierarchicalContextMenu',
  component: HierarchicalContextMenu,
  argTypes: {
    onSelect: { control: false },
    children: { control: false },
  },
  args: {
    items: [
      { id: 'save', label: '저장' },
      { id: 'disabled', label: '현재 불가', disabled: true },
      { id: 'outputs', label: '출력', children: [{ id: 'device', label: 'MixRelay IO' }] },
    ],
    onSelect: fn(),
    children: <div>우클릭·long press로 명령 열기</div>,
    disabled: false,
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
          '우클릭과 long press 표본입니다. 동일 명령의 키보드 경로는 HierarchicalMenuButton 사용례를 따릅니다.',
      },
    },
  },
} satisfies Meta<typeof HierarchicalContextMenu>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const Empty: Story = { args: { items: [] } };
