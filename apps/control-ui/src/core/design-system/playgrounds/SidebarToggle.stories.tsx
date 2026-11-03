import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { SidebarToggle } from '../../ui/SidebarToggle';
import { Text } from '../../ui/Text';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/SidebarToggle',
  component: SidebarToggle,
  argTypes: {
    onToggle: { control: false },
  },
  args: { label: '그래프 목록 펼치기', opened: false, controls: 'sample-sidebar', onToggle: fn() },
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
        <div id="sample-sidebar" hidden={!local.opened}>
          <Text>그래프 목록 표본</Text>
        </div>
        <SidebarToggle
          {...local}
          onToggle={() => {
            local.onToggle();
            set({
              opened: !local.opened,
              label: local.opened ? '그래프 목록 펼치기' : '그래프 목록 접기',
            });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof SidebarToggle>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Opened: Story = { args: { opened: true, label: '그래프 목록 접기' } };
