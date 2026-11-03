import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { ChannelConnectionDialog } from '../../audio-graph/ChannelConnectionDialog';
import type { AudioGraphModel, AudioGraphNode } from '../../audio-graph/model';
import { i18n } from '../../i18n';
import { ActionButton } from '../../ui/ActionButton';

function node(id: string, title: string, inputs: number, outputs: number): AudioGraphNode {
  const ports = (prefix: string, label: string, count: number) =>
    Array.from({ length: count }, (_, index) => ({ id: `${prefix}${index + 1}`, label }));
  return {
    id,
    position: { x: 0, y: 0 },
    inputPorts: ports('in', 'Input', inputs),
    outputPorts: ports('out', 'Output', outputs),
    appearance: { category: 'Fixture', detail: id, icon: 'A', title },
  };
}

const studio: AudioGraphModel = {
  revision: 1,
  nodes: [
    node('music', 'Music', 0, 4),
    node('stream', 'Stream mix', 2, 2),
    node('monitor', 'Headphones', 2, 0),
    node('monitor-b', 'Headphones', 2, 0),
  ],
  edges: [
    {
      id: 'music-stream',
      sourceNodeId: 'music',
      sourcePortId: 'out1',
      targetNodeId: 'stream',
      targetPortId: 'in1',
    },
  ],
};

const meta = {
  title: 'Core/Playgrounds/ChannelConnectionDialog',
  component: ChannelConnectionDialog,
  argTypes: {
    model: { control: false },
    onCancel: { control: false },
    onConnect: { control: false },
  },
  args: {
    model: studio,
    sourceNodeId: 'music',
    onCancel: fn(),
    onConnect: fn(),
  },
  parameters: {
    docs: {
      source: { type: 'dynamic' },
      description: {
        component:
          '그래프 fixture에서 실행합니다. 연결 결과는 onConnect 기록만 남기며 실제 오디오 그래프를 바꾸지 않습니다.',
      },
    },
  },
  // React state, not Storybook hooks: a Storybook hook update re-renders the story and
  // clears the argument spies before the play can read them.
  render: function Interactive(args) {
    const [visible, setVisible] = useState(true);
    return visible ? (
      <ChannelConnectionDialog
        {...args}
        onCancel={() => {
          args.onCancel();
          setVisible(false);
        }}
        onConnect={(connections) => {
          args.onConnect(connections);
          setVisible(false);
        }}
      />
    ) : (
      <ActionButton onClick={() => setVisible(true)}>채널 연결 열기</ActionButton>
    );
  },
} satisfies Meta<typeof ChannelConnectionDialog>;
export default meta;
type Story = StoryObj<typeof meta>;

// Plays read the active locale, so they hold under every Storybook language.
const text = (key: string, values?: Record<string, unknown>) =>
  i18n.t(`routing.connect.${key}`, values);
const field = (body: ReturnType<typeof within>, key: string) =>
  body.getByRole('button', { name: (name: string) => name.endsWith(text(key)) });

async function chooseOption(body: ReturnType<typeof within>, key: string, option: string) {
  await userEvent.click(field(body, key));
  await userEvent.click(await body.findByRole('option', { name: option }));
}

export const Playground: Story = {
  play: async ({ args, canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    const dialog = await body.findByRole('dialog', { name: text('title') });
    await waitFor(() =>
      expect(dialog.contains(canvasElement.ownerDocument.activeElement)).toBe(true),
    );
    await expect(body.getByRole('button', { name: text('submit') })).toBeDisabled();
    await chooseOption(body, 'target', 'Headphones (1)');
    await expect(body.getByRole('status')).toHaveTextContent(
      text('summary', { accepted: 2, skipped: 2 }),
    );
    await userEvent.click(body.getByRole('button', { name: text('submit') }));
    await expect(args.onConnect).toHaveBeenCalledOnce();
    await waitFor(() => expect(body.queryByRole('dialog')).not.toBeInTheDocument());
  },
};

export const SelectedChannels: Story = {
  args: { sourcePortIds: ['out2', 'out4'] },
  play: async ({ canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    const dialog = await body.findByRole('dialog');
    await expect(dialog).toHaveTextContent(text('selected', { channels: 'CH 2, CH 4' }));
    await userEvent.click(body.getByRole('button', { name: text('useRange') }));
    await expect(field(body, 'count')).toHaveTextContent('3');
  },
};

export const SkippedChannels: Story = {
  play: async ({ canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    await chooseOption(body, 'target', 'Stream mix');
    const preview = within(body.getByRole('list', { name: text('preview') }));
    const skipped = (source: string, reason: string) =>
      text('skipped', { source, reason: text(`reasons.${reason}`) });
    await expect(preview.getByText(skipped('CH 1', 'existing'))).toBeInTheDocument();
    await expect(preview.getByText(skipped('CH 3', 'out-of-range'))).toBeInTheDocument();
  },
};

export const NoTargets: Story = {
  args: { model: { revision: 1, nodes: [node('music', 'Music', 0, 2)], edges: [] } },
  play: async ({ canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    await expect(await body.findByText(text('noTargets'))).toBeInTheDocument();
    await expect(body.getByRole('button', { name: text('submit') })).toBeDisabled();
  },
};
