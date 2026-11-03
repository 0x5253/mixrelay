import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { CollectionItem } from '../../ui/CollectionItem';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/CollectionItem',
  component: CollectionItem,
  argTypes: {
    onSelect: { control: false },
    onRemove: { control: false },
  },
  args: {
    name: 'Studio',
    selected: false,
    disabled: false,
    removeLabel: '삭제 확인',
    onSelect: fn(),
    onRemove: fn(),
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
        component: '삭제 callback은 Actions에만 기록합니다. 데이터는 삭제하지 않습니다.',
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
        <CollectionItem
          {...local}
          onSelect={() => {
            local.onSelect();
            set({ selected: true });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof CollectionItem>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Selected: Story = { args: { selected: true } };
export const Disabled: Story = { args: { disabled: true } };
export const LongName: Story = {
  args: { name: 'Studio · 매우 긴 그래프 이름 · 긴 번역에서도 삭제 명령 유지' },
};
