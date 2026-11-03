import type { Meta, StoryObj } from '@storybook/react-vite';
import { ProductHeading } from '../../ui/BrandIdentity';

const meta = {
  title: 'Core/Playgrounds/ProductHeading',
  component: ProductHeading,
  argTypes: {},
  args: {},
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
        component: '공개 props가 없습니다. 실제 BrandIdentity를 제목 h1 안에 렌더합니다.',
      },
    },
  },
} satisfies Meta<typeof ProductHeading>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Dark: Story = { globals: { theme: 'dark' }, args: {} };
