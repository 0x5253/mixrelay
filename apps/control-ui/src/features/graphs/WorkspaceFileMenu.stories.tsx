import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { i18n } from '../../core/i18n';
import { WorkspaceFileMenu } from './WorkspaceFileMenu';

function sampleControlPort() {
  return new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
}
/** An engine connection that offers no workspace file commands. */
const base = {};
const port = Object.assign(sampleControlPort(), { workspaceFile: fn(async () => null) });
const meta = {
  title: 'Features/Graphs/File Menu',
  component: WorkspaceFileMenu,
  args: { controlPort: port, disabled: false, run: fn(async () => undefined) },
  argTypes: { controlPort: { control: false }, run: { control: false } },
  parameters: {
    docs: {
      description: {
        component:
          '실제 WorkspaceFileMenu. 새 문서·불러오기·저장·다른 이름 저장은 callback만 기록하며 native 파일 선택이나 파일 쓰기를 실행하지 않습니다.',
      },
    },
  },
} satisfies Meta<typeof WorkspaceFileMenu>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Available: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const NoCapability: Story = { args: { controlPort: base } };
export const KeyboardSave: Story = {
  play: async ({ canvasElement, args }) => {
    await userEvent.tab();
    const trigger = within(canvasElement).getByRole('button', {
      name: i18n.t('workspaceFile.file'),
    });
    await expect(trigger).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    const item = await within(canvasElement.ownerDocument.body).findByRole('menuitem', {
      name: i18n.t('workspaceFile.save'),
    });
    await userEvent.click(item);
    await expect(args.run).toHaveBeenCalledWith('save', i18n.resolvedLanguage ?? i18n.language);
  },
};
