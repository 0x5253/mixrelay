import type { Meta, StoryObj } from '@storybook/react-vite';
import { DisclosureList } from '../../ui/DisclosureList';
import { Stack } from '../../ui/Stack';
import { Text } from '../../ui/Text';

const item = (index: number, title: string) => ({
  id: String(index),
  title,
  content: (
    <Stack>
      <Text kind="supporting">
        이 플러그인을 불러오지 못했습니다. 다른 이펙트는 계속 사용할 수 있습니다.
      </Text>
      <Text kind="caption">vst3:example:{index} · Native process exited with code 1.</Text>
    </Stack>
  ),
});

const meta = {
  title: 'Core/Playgrounds/DisclosureList',
  component: DisclosureList,
  argTypes: { items: { control: false } },
  args: {
    items: [
      item(1, 'VST3 · Example Effect · 검색'),
      item(2, 'VST3 · Example Effect 2 · 플러그인 확인'),
      item(3, 'CLAP · 검색'),
    ],
  },
  parameters: {
    docs: {
      source: { type: 'dynamic' },
      description: {
        component:
          '반복되는 세부 항목을 한 줄 행으로 접습니다. 펼친 내용은 제목 글자선에서 시작하고 행 아래 같은 간격으로 닫힙니다.',
      },
    },
  },
} satisfies Meta<typeof DisclosureList>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const LongTitles: Story = {
  args: {
    items: [
      item(1, 'VST3 · Unavailable Effect With A Very Long Vendor And Product Name · 플러그인 확인'),
      item(2, 'AU · Другой очень длинный эффект с переводом · 검색'),
    ],
  },
};
