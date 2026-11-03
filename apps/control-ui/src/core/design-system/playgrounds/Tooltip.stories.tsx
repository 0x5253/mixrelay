import type { Meta, StoryObj } from '@storybook/react-vite';
import { Tooltip } from '../../ui/Tooltip';

const meta = {
  title: 'Core/Playgrounds/Tooltip',
  component: Tooltip,
  argTypes: {
    position: { control: 'select', options: ['top', 'bottom', 'left', 'right'] },
    kind: { control: 'inline-radio', options: ['label', 'readout'] },
    children: { control: false },
  },
  args: {
    label: '현재 값을 확인합니다',
    position: 'top',
    delay: 350,
    offset: 4,
    kind: 'label',
    nonInteractive: false,
    children: <button type="button">설명 대상</button>,
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
          'hover·키보드 focus로 엽니다. nonInteractive 표본은 Tab 대상을 추가하지 않습니다.',
      },
    },
  },
} satisfies Meta<typeof Tooltip>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Opened: Story = { args: { opened: true } };
export const NonInteractive: Story = {
  args: { nonInteractive: true, children: <span>읽기 전용 설명 대상</span> },
};
export const Right: Story = { args: { position: 'right' } };
export const Readout: Story = {
  args: {
    kind: 'readout',
    label: 'CH 3',
    position: 'right',
    offset: 7,
    opened: true,
    nonInteractive: true,
    children: (
      <span role="img" aria-label="Main 3, CH 3">
        ●
      </span>
    ),
  },
};
