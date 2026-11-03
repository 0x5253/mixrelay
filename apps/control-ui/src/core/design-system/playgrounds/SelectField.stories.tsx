import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { SelectField } from '../../ui/SelectField';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/SelectField',
  component: SelectField,
  argTypes: {
    density: { control: 'select', options: ['regular', 'compact'] },
    labelPlacement: { control: 'select', options: ['above', 'inline'] },
    onChange: { control: false },
  },
  args: {
    data: [
      { value: 'stereo', label: 'Stereo', description: 'L → L, R → R' },
      { value: 'mono', label: 'Mono sum', description: '(L + R) / 2' },
      { value: 'unavailable', label: '미지원', disabled: true },
    ],
    value: 'stereo',
    label: '채널 처리',
    disabled: false,
    onChange: fn(),
    density: 'regular',
    labelPlacement: 'above',
    error: '',
    placeholder: '선택',
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
        <SelectField
          {...local}
          onChange={(value) => {
            local.onChange(value);
            set({ value });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof SelectField>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Invalid: Story = { args: { error: '설정이 적용되지 않았습니다.' } };
export const Disabled: Story = { args: { disabled: true } };
export const Empty: Story = { args: { value: null, data: [] } };

export const Selection: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: 'Stereo 채널 처리' }));
    const body = within(canvasElement.ownerDocument.body);
    await userEvent.click(await body.findByRole('option', { name: 'Mono sum' }));
    await waitFor(() =>
      expect(canvas.getByRole('button', { name: 'Mono sum 채널 처리' })).toBeVisible(),
    );
    await userEvent.tab();
    await waitFor(() =>
      expect(canvas.getByRole('button', { name: 'Mono sum 채널 처리' })).toBeVisible(),
    );
  },
};
