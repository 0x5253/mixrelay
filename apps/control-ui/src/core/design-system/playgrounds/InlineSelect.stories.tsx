import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { InlineSelect } from '../../ui/InlineSelect';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/InlineSelect',
  component: InlineSelect,
  argTypes: {
    onChange: { control: false },
  },
  args: {
    label: '채널',
    value: '2',
    data: [
      { value: '1', label: '1 CH' },
      { value: '2', label: '2 CH' },
    ],
    disabled: false,
    onChange: fn(),
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
        <InlineSelect
          {...local}
          onChange={(value) => {
            local.onChange(value);
            set({ value });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof InlineSelect>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { disabled: true } };
