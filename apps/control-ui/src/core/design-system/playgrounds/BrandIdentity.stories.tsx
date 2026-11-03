import type { Meta, StoryObj } from '@storybook/react-vite';
import { BrandIdentity } from '../../ui/BrandIdentity';

const meta = {
  title: 'Core/Playgrounds/BrandIdentity',
  component: BrandIdentity,
  argTypes: {
    tone: { control: 'select', options: ['primary', 'neutral', 'inverse'] },
    size: { control: 'select', options: ['compact', 'standard', 'display'] },
  },
  args: { size: 'compact', tone: 'primary', outline: false },
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
} satisfies Meta<typeof BrandIdentity>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Display: Story = { args: { size: 'display' } };
export const Outline: Story = { args: { outline: true } };
export const Neutral: Story = { args: { tone: 'neutral' } };
