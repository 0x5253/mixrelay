import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { HierarchicalMenuButton } from '../../ui/HierarchicalMenu';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/HierarchicalMenuButton',
  component: HierarchicalMenuButton,
  argTypes: {
    onSelect: { control: false },
    onOpenedChange: { control: false },
  },
  args: {
    items: [
      { id: 'save', label: '저장' },
      { id: 'disabled', label: '현재 불가', disabled: true },
      { id: 'outputs', label: '출력', children: [{ id: 'device', label: 'MixRelay IO' }] },
    ],
    onSelect: fn(),
    label: '파일 명령',
    buttonContent: '메뉴',
    opened: false,
    onOpenedChange: fn(),
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
        <HierarchicalMenuButton
          {...local}
          onOpenedChange={(opened) => {
            local.onOpenedChange?.(opened);
            set({ opened });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof HierarchicalMenuButton>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Opened: Story = { args: { opened: true } };
export const Empty: Story = { args: { items: [] } };
