import type { Meta, StoryObj } from '@storybook/react-vite';
import { BrandIdentity, ProductHeading } from './BrandIdentity';
import { ContentSection } from './ContentSection';
import { Inline } from './Inline';
import { Stack } from './Stack';
import { Text } from './Text';

const meta = {
  title: 'Core/Identity/Brand',
  component: BrandIdentity,
} satisfies Meta<typeof BrandIdentity>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Primary: Story = { args: { size: 'standard' } };
export const Monochrome: Story = { args: { size: 'standard', tone: 'neutral' } };
export const Inverse: Story = {
  args: { size: 'standard', tone: 'inverse' },
  decorators: [
    (Story) => (
      <div style={{ background: 'var(--background)', padding: 'var(--space-4)' }}>
        <Story />
      </div>
    ),
  ],
  globals: { theme: 'dark' },
};
export const Outline: Story = { args: { outline: true } };
export const ProductHeader: Story = {
  render: () => (
    <Stack>
      <ProductHeading />
      <BrandIdentity size="standard" />
      <Text kind="caption">
        제품 이름은 같은 접근성 이름을 유지하며 Tab 대상을 추가하지 않습니다.
      </Text>
    </Stack>
  ),
};
export const NarrowLongTranslation: Story = {
  render: () => (
    <div style={{ width: 240, maxWidth: '100%' }}>
      <ContentSection title="MixRelay 제품 아이덴티티와 매우 긴 번역 설명">
        <Stack>
          <BrandIdentity />
          <Inline>
            <BrandIdentity />
            <BrandIdentity outline />
          </Inline>
          <Text kind="supporting">
            로고는 번역하지 않습니다. 주변의 길어진 문구는 로고 아래에서 줄바꿈하고 제품 이름과
            섞이지 않습니다.
          </Text>
        </Stack>
      </ContentSection>
    </div>
  ),
};
