import type { Meta, StoryObj } from '@storybook/react-vite';
import { type ComponentProps, useState } from 'react';
import { expect, userEvent, within } from 'storybook/test';
import type { MidiStatus } from '../core/domain/midi';
import { i18n } from '../core/i18n';
import { midiBinding, midiFixture, midiPortFixture } from '../features/midi/midiFixtures';
import { App } from './App';
import workspaceMeta, { MidiControls } from './App.stories';

const binding = {
  ...midiBinding,
  target: { graphId: 'graph-1', nodeId: 'trim' },
  graphName: 'Untitled Graph',
  nodeName: 'Monitor trim',
};
const mapped: MidiStatus = {
  ...midiFixture,
  bindings: [binding],
  targets: [{ target: binding.target, label: 'Untitled Graph / Monitor trim' }],
};
function MidiWorkspace({ status, ...args }: ComponentProps<typeof App> & { status: MidiStatus }) {
  const [port] = useState(() => midiPortFixture(status));
  return <App {...args} midiPort={port} />;
}
const meta = {
  ...workspaceMeta,
  title: 'Features/MIDI/Controls',
  component: MidiWorkspace,
  args: { ...workspaceMeta.args, status: { ...mapped, bindings: [] } },
  render: (args, { loaded }) => <MidiWorkspace {...args} controlPort={loaded.controlPort} />,
  globals: { locale: 'en' },
} satisfies Meta<typeof MidiWorkspace>;
export default meta;
type Story = StoryObj<typeof meta>;
export const MapModeOff: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole('group', { name: 'Monitor trim' });
    await expect(
      canvas.getByRole('complementary', { name: i18n.t('studio.graphs') }),
    ).toBeVisible();
    await expect(
      canvas.queryByRole('complementary', { name: i18n.t('midi.title') }),
    ).not.toBeInTheDocument();
  },
};
export const MapModeOn: Story = { play: MidiControls.play };
export const SelectedTargetWaiting: Story = {
  play: async (context) => {
    await MapModeOn.play?.(context);
    const canvas = within(context.canvasElement);
    const target = await canvas.findByRole('button', {
      name: i18n.t('midi.mapTarget', {
        name: 'Monitor trim',
        action: i18n.t('midi.actions.volume'),
      }),
    });
    await userEvent.click(target);
    await expect(target).toHaveAttribute('aria-pressed', 'true');
  },
};
export const MappedControls: Story = {
  args: { status: mapped },
  play: MidiControls.play,
};
export const DarkMapMode: Story = { ...MappedControls, globals: { locale: 'en', theme: 'dark' } };
export const OneControlSeveralTargets: Story = {
  ...MappedControls,
  args: {
    status: {
      ...mapped,
      bindings: [
        binding,
        { ...binding, target: { graphId: 'graph-1', nodeId: 'app' }, nodeName: 'QuickTime Player' },
      ],
      targets: [
        ...mapped.targets,
        {
          target: { graphId: 'graph-1', nodeId: 'app' },
          label: 'Untitled Graph / QuickTime Player',
        },
      ],
    },
  },
};
export const TargetMissing: Story = {
  ...MappedControls,
  args: {
    status: {
      ...mapped,
      bindings: [
        {
          ...binding,
          target: { graphId: 'graph-1', nodeId: 'missing' },
          nodeName: 'Removed microphone',
        },
      ],
      targets: [],
    },
  },
};
export const MidiUnavailable: Story = {
  ...MappedControls,
  args: {
    status: {
      ...mapped,
      unavailable: 'ALSA sequencer: permission denied',
      devices: [],
    },
  },
};
export const NarrowLongTranslation: Story = {
  ...OneControlSeveralTargets,
  globals: { locale: 'pseudo', viewport: { value: 'midiNarrow' } },
  parameters: {
    viewport: {
      options: { midiNarrow: { name: 'MIDI narrow', styles: { width: '420px', height: '860px' } } },
    },
  },
};
export const NarrowLongTranslationOff: Story = {
  ...NarrowLongTranslation,
  play: MapModeOff.play,
};
export const Keyboard: Story = {
  ...MappedControls,
  play: async (context) => {
    await MappedControls.play?.(context);
    const canvas = within(context.canvasElement);
    const row = await canvas.findByRole('button', {
      name: i18n.t('midi.selectMapping', {
        name: 'Untitled Graph / Monitor trim',
        action: i18n.t('midi.actions.volume'),
      }),
    });
    row.focus();
    await userEvent.keyboard('{Enter}{Tab}');
    await expect(
      canvas.getByRole('button', { name: new RegExp(i18n.t('midi.mode')) }),
    ).toHaveFocus();
  },
};
