import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { i18n } from '../../core/i18n';
import { RoutingWorkspace } from './RoutingWorkspace';

const graph: RuntimeGraphDocument = {
  revision: 1,
  schemaVersion: 2,
  edges: [],
  nodes: ['firefox-video', 'firefox-meeting'].map((id) => ({
    id,
    label: 'Firefox',
    type: 'application_capture',
    config: {
      gainDb: 0,
      selector: { bundle_id: 'org.mozilla.firefox', include_descendants: true, match: 'bundle_id' },
      sourcePlayback: 'preserve',
      stream: { channelLayout: 'stereo', channels: 2 },
    },
  })),
};
function createFixture() {
  const port = new StorybookMockControlPort({
    graph,
    graphStatus: { activeRevision: 1, submittedRevision: 1, protocolVersion: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      bufferFrames: 128,
      sampleRate: 48000,
      overruns: 0,
      underruns: 0,
    },
    meters: { protocolVersion: 1, revision: 1, nodes: [] },
  });
  const audio = fn(port.applyGraph.bind(port));
  const write = fn(port.setGraphLayout.bind(port));
  port.applyGraph = audio;
  port.setGraphLayout = write;
  return { port, audio, write };
}

const meta = {
  title: 'Features/Routing/Node Display Names',
  component: RoutingWorkspace,
  args: {
    controlPort: new StorybookMockControlPort({
      graph,
      graphStatus: { activeRevision: 1, submittedRevision: 1, protocolVersion: 1 },
      status: {
        protocolVersion: 1,
        driverState: 'ready',
        bufferFrames: 128,
        sampleRate: 48000,
        overruns: 0,
        underruns: 0,
      },
      meters: { protocolVersion: 1, revision: 1, nodes: [] },
    }),
    title: 'Firefox instances',
  },
  argTypes: { controlPort: { control: false }, graphId: { control: false } },
  decorators: [
    (Story) => (
      <div style={{ height: 700 }}>
        <Story />
      </div>
    ),
  ],
  parameters: {
    docs: {
      description: {
        component:
          '같은 Firefox selector를 가진 그래프 인스턴스 두 개입니다. 별칭은 표시 메타데이터에만 저장하며 창·탭별 캡처를 만들지 않습니다. 표본은 실제 RoutingWorkspace와 메모리 preview 포트입니다.',
      },
    },
  },
  loaders: [() => ({ fixture: createFixture() })],
  render: function Instances(args, context) {
    const fixture = context.loaded.fixture as ReturnType<typeof createFixture>;
    return (
      <RoutingWorkspace key={context.id} {...args} controlPort={fixture.port} graphId="graph-1" />
    );
  },
} satisfies Meta<typeof RoutingWorkspace>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Instances: Story = {};
export const OriginalPlaceholder: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const nodes = await canvas.findAllByRole('group', { name: 'Firefox' });
    const first = nodes[0];
    if (!first) throw new Error('Missing Firefox instance');
    first.focus();
    await userEvent.keyboard('{F2}');
    const body = within(canvasElement.ownerDocument.body);
    const input = await body.findByRole('textbox', { name: i18n.t('routing.nodeName.label') });
    await expect(input).toHaveValue('');
    await expect(input).toHaveAttribute('placeholder', 'Firefox');
    await expect(input).not.toHaveAccessibleDescription();
    await userEvent.keyboard('{Escape}');
    const add = canvas.getByRole('button', { name: i18n.t('routing.addNode') });
    await expect(add.textContent?.trim()).toBe('+');
    add.focus();
    await waitFor(() =>
      expect(body.getByRole('tooltip')).toHaveTextContent(i18n.t('routing.addNode')),
    );
    await userEvent.keyboard('{Enter}');
    await waitFor(() =>
      expect(body.getByRole('menu', { name: i18n.t('routing.addNode') })).toBeVisible(),
    );
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(add).toHaveFocus());
    first.focus();
    await userEvent.keyboard('{F2}');
    await waitFor(() =>
      expect(body.getByRole('textbox', { name: i18n.t('routing.nodeName.label') })).toHaveFocus(),
    );
  },
};
export const Rename: Story = {
  play: async ({ canvasElement, loaded }) => {
    const canvas = within(canvasElement);
    const nodes = await canvas.findAllByRole('group', { name: 'Firefox' });
    const first = nodes[0];
    if (!first) throw new Error('Missing Firefox instance');
    await userEvent.dblClick(within(first).getByText('Firefox', { exact: true }));
    const body = within(canvasElement.ownerDocument.body);
    const input = await body.findByRole('textbox', { name: i18n.t('routing.nodeName.label') });
    await userEvent.clear(input);
    await userEvent.type(input, '영상용');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(canvas.getByRole('group', { name: '영상용' })).toBeVisible());
    await waitFor(() => expect(canvas.getByRole('group', { name: '영상용' })).toHaveFocus());
    const second = canvas.getByRole('group', { name: 'Firefox' });
    second.focus();
    await userEvent.keyboard('{F2}');
    const next = await body.findByRole('textbox', { name: i18n.t('routing.nodeName.label') });
    await userEvent.clear(next);
    await userEvent.type(next, '회의용');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(canvas.getByRole('group', { name: '회의용' })).toBeVisible());
    await waitFor(() => expect(canvas.getByRole('group', { name: '회의용' })).toHaveFocus());

    const fixture = loaded.fixture as ReturnType<typeof createFixture>;
    await waitFor(async () =>
      expect((await fixture.port.getGraphLayout('graph-1')).layout.nodeLabels).toEqual({
        'firefox-video': '영상용',
        'firefox-meeting': '회의용',
      }),
    );
    await expect(fixture.write).toHaveBeenCalled();
    await expect(fixture.audio).not.toHaveBeenCalled();
    await expect(await fixture.port.getGraph()).toEqual(graph);
  },
};
export const Reset: Story = {
  play: async (context) => {
    await Rename.play?.(context);
    const node = within(context.canvasElement).getByRole('group', { name: '영상용' });
    node.focus();
    await userEvent.keyboard('{F2}');
    const input = await within(context.canvasElement.ownerDocument.body).findByRole('textbox', {
      name: i18n.t('routing.nodeName.label'),
    });
    await userEvent.clear(input);
    await userEvent.keyboard('{Enter}');
    await waitFor(() =>
      expect(within(context.canvasElement).getByRole('group', { name: 'Firefox' })).toBeVisible(),
    );
    await expect(
      within(context.canvasElement).getByRole('group', { name: '회의용' }),
    ).toBeVisible();
  },
};
export const Cancel: Story = {
  play: async ({ canvasElement }) => {
    const nodes = await within(canvasElement).findAllByRole('group', {
      name: 'Firefox',
    });
    const node = nodes[0];
    if (!node) throw new Error('Missing Firefox instance');
    node.focus();
    await userEvent.keyboard('{F2}');
    const input = await within(canvasElement.ownerDocument.body).findByRole('textbox', {
      name: i18n.t('routing.nodeName.label'),
    });
    await userEvent.clear(input);
    await userEvent.type(input, '취소할 이름');
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(node).toHaveFocus());
    await expect(
      within(canvasElement).queryByRole('group', { name: '취소할 이름' }),
    ).not.toBeInTheDocument();
  },
};
export const NarrowLongTranslation: Story = {
  globals: { locale: 'pseudo' },
  decorators: [
    (Story) => (
      <div style={{ width: 390, maxWidth: '100%', height: 700 }}>
        <Story />
      </div>
    ),
  ],
};
