import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { NodeOptionsSection } from '../../ui/NodeOptionsSection';
import { TextField } from '../../ui/TextField';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/NodeOptionsSection',
  component: NodeOptionsSection,
  argTypes: {
    children: { control: false },
    onToggle: { control: false },
  },
  args: {
    label: '노드 세부 설정',
    opened: false,
    disabled: false,
    description: '세부 값은 실제 노드 계약을 소비합니다.',
    children: <TextField label="설정 이름" defaultValue="Studio" />,
    onToggle: fn(),
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
  render: function Interactive(args) {
    const { local, set, commit } = usePlaygroundState(args);
    return (
      <fieldset
        aria-label="Playground 예제"
        onBlur={commit}
        style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
      >
        <NodeOptionsSection
          {...local}
          onToggle={() => {
            local.onToggle();
            set({ opened: !local.opened });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof NodeOptionsSection>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Opened: Story = { args: { opened: true } };
export const Inert: Story = { args: { opened: true, disabled: true } };
