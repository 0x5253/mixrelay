import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import type { ProductUpdate } from '../../core/domain/updates';
import type { UpdatesPort } from '../../core/ports/UpdatesPort';
import { Text } from '../../core/ui/Text';
import { UpdateAccess } from './UpdateAccess';

const unpublished: ProductUpdate[] = ['mix-relay'].map((product) => ({
  product: product as ProductUpdate['product'],
  installedVersion: '0.1.0',
  latestVersion: null,
  state: 'not_published',
  downloads: [],
  notesUrl: null,
}));
const releases: ProductUpdate[] = unpublished.map((product) => ({
  ...product,
  latestVersion: '0.1.1',
  state: 'available',
  downloads: [
    {
      os: 'macos',
      arch: 'universal',
      url: 'https://downloads.example.test/sample.pkg',
      sha256: 'a'.repeat(64),
      size: 1024,
    },
  ],
}));
const fixture = (products = unpublished): UpdatesPort => ({
  check: fn(async () => structuredClone(products)),
  openDownload: fn(async () => undefined),
});
const meta = {
  title: 'Features/Updates/UpdateAccess',
  component: UpdateAccess,
  args: {
    port: fixture(),
    children: (action) => (
      <header>
        <Text>업데이트는 헤더에서 확인합니다.</Text>
        {action}
      </header>
    ),
  },
  argTypes: { port: { control: false }, children: { control: false } },
  parameters: {
    docs: {
      description: {
        component:
          '실제 UpdateAccess와 결정적 UpdatesPort. 샘플 다운로드 버튼은 요청을 기록하며 URL을 열거나 파일을 설치하지 않습니다.',
      },
    },
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole('button'));
    const dialog = await within(canvasElement.ownerDocument.body).findByRole('dialog');
    await waitFor(() => expect(dialog).toBeVisible());
  },
} satisfies Meta<typeof UpdateAccess>;
export default meta;
type Story = StoryObj<typeof meta>;
export const NotPublished: Story = {};
export const Available: Story = { args: { port: fixture(releases) } };
export const Current: Story = {
  args: {
    port: fixture(
      releases.map((product) => ({ ...product, latestVersion: '0.1.0', state: 'current' })),
    ),
  },
};
export const Checking: Story = {
  args: {
    port: {
      check: fn(() => new Promise<ProductUpdate[]>(() => undefined)),
      openDownload: fn(async () => undefined),
    },
  },
};
export const Failed: Story = {
  args: {
    port: {
      check: fn(async () => {
        throw new Error('Fixture catalog failure');
      }),
      openDownload: fn(async () => undefined),
    },
  },
};
export const OpenFailed: Story = {
  args: {
    port: {
      ...fixture(releases),
      openDownload: fn(async () => {
        throw new Error('Fixture open failure');
      }),
    },
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole('button'));
    const dialog = await within(canvasElement.ownerDocument.body).findByRole('dialog');
    const downloads = await within(dialog).findAllByRole('button', { name: /PKG/ });
    const download = downloads[0];
    if (!download) throw new Error('Update fixture requires a download');
    await userEvent.click(download);
    const alert = await within(dialog).findByRole('alert');
    await waitFor(() => expect(alert).toBeVisible());
  },
};
export const NoPort: Story = {
  args: { port: null },
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).queryByRole('button')).not.toBeInTheDocument();
    await expect(within(canvasElement).getByText('업데이트는 헤더에서 확인합니다.')).toBeVisible();
  },
};
export const NarrowLongTranslation: Story = {
  ...Available,
  globals: { locale: 'pseudo' },
  decorators: [
    (Story) => (
      <div style={{ width: 390, maxWidth: '100%' }}>
        <Story />
      </div>
    ),
  ],
};

export const Published: Story = {
  args: {
    port: fixture(
      releases.map((product) => ({ ...product, installedVersion: null, state: 'published' })),
    ),
  },
};
