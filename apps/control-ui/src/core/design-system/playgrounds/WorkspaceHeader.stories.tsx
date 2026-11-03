import type { Meta, StoryObj } from '@storybook/react-vite';
import { TitleField } from '../../ui/TitleField';
import { WorkspaceHeader } from '../../ui/WorkspaceHeader';

const meta = {
  title: 'Core/Playgrounds/WorkspaceHeader',
  component: WorkspaceHeader,
  argTypes: {
    children: { control: false },
  },
  args: { children: <TitleField aria-label="그래프 이름" defaultValue="Studio" /> },
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
} satisfies Meta<typeof WorkspaceHeader>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const LongName: Story = {
  args: {
    children: (
      <TitleField
        aria-label="그래프 이름"
        defaultValue="아주 긴 그래프 이름 · Studio Monitor Mix"
      />
    ),
  },
};
