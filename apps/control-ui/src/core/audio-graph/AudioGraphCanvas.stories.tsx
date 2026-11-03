import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { expect, userEvent, within } from 'storybook/test';

import mixrelayAppIcon from '../../../../desktop/src-tauri/icons/32x32.png';
import { AudioGraphCanvas, type AudioGraphNodeMenuItem } from './AudioGraphCanvas';
import quickTimeAppIcon from './fixtures/quicktime-player.png';
import type { AudioGraphModel, AudioGraphPort } from './model';

const broadcastMix: AudioGraphModel = {
  revision: 1,
  nodes: [
    {
      id: 'chrome',
      position: { x: 40, y: 80 },
      inputPorts: [],
      outputPorts: [{ id: 'audio', label: 'Chrome audio output' }],
      appearance: {
        category: 'Source',
        detail: 'Chrome · Tab audio',
        icon: 'APP',
        status: 'Live',
        title: 'Chrome',
      },
    },
    {
      id: 'microphone',
      position: { x: 40, y: 260 },
      inputPorts: [],
      outputPorts: [{ id: 'audio', label: 'Microphone output' }],
      appearance: {
        category: 'Source',
        detail: 'Scarlett 2i2 · Input 1',
        icon: 'MIC',
        title: 'Microphone',
      },
    },
    {
      id: 'mixer',
      position: { x: 390, y: 150 },
      inputPorts: [
        { id: 'chrome', label: 'Chrome mixer input' },
        { id: 'microphone', label: 'Microphone mixer input' },
      ],
      outputPorts: [{ id: 'audio', label: 'Mixer output' }],
      appearance: {
        category: 'Processor',
        detail: 'Chrome + microphone',
        icon: 'MIX',
        title: 'Broadcast mix',
      },
    },
    {
      id: 'stream',
      position: { x: 750, y: 150 },
      inputPorts: [{ id: 'to_os', label: 'Virtual device input' }],
      outputPorts: [],
      appearance: {
        category: 'Output',
        detail: 'MixRelay Stream · 48 kHz',
        icon: 'IO',
        status: 'Ready',
        title: 'OBS Stream',
      },
    },
  ],
  edges: [
    {
      id: 'chrome-to-mixer',
      sourceNodeId: 'chrome',
      sourcePortId: 'audio',
      targetNodeId: 'mixer',
      targetPortId: 'chrome',
    },
    {
      id: 'microphone-to-mixer',
      sourceNodeId: 'microphone',
      sourcePortId: 'audio',
      targetNodeId: 'mixer',
      targetPortId: 'microphone',
    },
    {
      id: 'mixer-to-stream',
      sourceNodeId: 'mixer',
      sourcePortId: 'audio',
      targetNodeId: 'stream',
      targetPortId: 'to_os',
    },
  ],
};

const meta = {
  title: 'Core/Audio Graph/Canvas',
  component: AudioGraphCanvas,
  args: {
    ariaLabel: 'MixRelay audio routing playground',
    model: broadcastMix,
    onModelChange: () => undefined,
  },
  parameters: { layout: 'fullscreen' },
  render: () => <InteractiveGraph />,
} satisfies Meta<typeof AudioGraphCanvas>;

export default meta;
type Story = StoryObj<typeof meta>;

export const ConnectDisconnectReconnect: Story = {};

export const ReleaseAcceptance: Story = {
  name: 'Release acceptance · real ReactFlow gestures',
  render: () => <ReleaseAcceptanceGraph />,
};

export const ResizedOptions: Story = {
  render: () => <InteractiveGraph initialModel={resizedOptionsFixture} />,
};

export const ResizedProviderOptions: Story = {
  render: () => <InteractiveGraph initialModel={resizedProviderOptionsFixture} />,
};

export const OverlappingOptions: Story = {
  render: () => {
    const source = resizedOptionsFixture.nodes[0];
    if (!source) throw new Error('Overlapping options require a source node.');
    return (
      <InteractiveGraph
        initialModel={{
          ...resizedOptionsFixture,
          nodes: [
            {
              ...source,
              size: undefined,
              position: { x: 60, y: 60 },
            },
            {
              ...source,
              id: 'overlapping-source',
              size: undefined,
              position: { x: 60, y: 225 },
              appearance: {
                ...source.appearance,
                title: 'Overlapping source',
              },
            },
          ],
        }}
      />
    );
  },
};

export const ChannelSelectionFeedback: Story = {
  render: () => <InteractiveGraph initialModel={channelSelectionFixture} />,
};

export const TwoApplicationSources: Story = {
  name: 'Two application sources · channel selection',
  render: () => <InteractiveGraph initialModel={twoApplicationSourcesFixture} />,
};

export const ChannelSelectionLongTranslation: Story = {
  globals: { locale: 'pseudo' },
  render: () => <InteractiveGraph initialModel={channelSelectionFixture} />,
};

export const PortTooltipTypography: Story = {
  render: () => <PortTooltipGraph />,
  play: async ({ canvasElement }) => {
    const port = await within(canvasElement).findByLabelText('Output channel 64, CH 64');
    await userEvent.hover(port);
    await expect(
      await within(canvasElement.ownerDocument.body).findByRole('tooltip'),
    ).toHaveTextContent('CH 64');
  },
};

export const PortTooltipNarrowLongTranslation: Story = {
  ...PortTooltipTypography,
  globals: { locale: 'pseudo' },
  render: () => <PortTooltipGraph narrow />,
};

function PortTooltipGraph({ narrow = false }: { narrow?: boolean }) {
  return (
    <div style={{ width: narrow ? 390 : '100%', maxWidth: '100%', height: 500 }}>
      <AudioGraphCanvas
        ariaLabel="64-channel port tooltip"
        model={{
          revision: 1,
          nodes: [
            {
              id: 'tooltip-source',
              position: { x: 40, y: 40 },
              size: { width: 280, height: 380 },
              inputPorts: [],
              outputPorts: channels('tooltip-source', 'output', 64),
              appearance: {
                category: 'Source',
                channelCount: 64,
                icon: 'IO',
                title: narrow
                  ? 'Sehr langer Gerätename · 入力チャンネル · 입력 채널 · Output channels'
                  : 'Output channels',
              },
            },
          ],
          edges: [],
        }}
        onModelChange={() => undefined}
      />
    </div>
  );
}

function InteractiveGraph({ initialModel = broadcastMix }: { initialModel?: AudioGraphModel }) {
  const [model, setModel] = useState(initialModel);
  return (
    <div style={{ width: '100%', height: '100dvh' }}>
      <AudioGraphCanvas
        ariaLabel="MixRelay audio routing playground"
        model={model}
        onModelChange={setModel}
        onNodeGainChange={(nodeId, valueDb) =>
          setModel((current) => ({
            ...current,
            nodes: current.nodes.map((node) =>
              node.id === nodeId && node.appearance.gainControl
                ? {
                    ...node,
                    appearance: {
                      ...node.appearance,
                      gainControl: { ...node.appearance.gainControl, valueDb },
                    },
                  }
                : node,
            ),
          }))
        }
      />
    </div>
  );
}

const channels = (nodeId: string, side: 'input' | 'output', count: number): AudioGraphPort[] =>
  Array.from({ length: count }, (_, channelIndex) => ({
    channelIndex,
    id: `${nodeId}-${side}-${channelIndex + 1}`,
    label: `${side === 'input' ? 'Input' : 'Output'} channel ${channelIndex + 1}`,
  }));

const resizedOptionsFixture: AudioGraphModel = {
  revision: 1,
  nodes: [
    {
      id: 'resized-source',
      position: { x: 90, y: 55 },
      size: { width: 300, height: 400 },
      inputPorts: [],
      outputPorts: channels('resized-source', 'output', 2),
      appearance: {
        category: 'Source',
        channelCount: 2,
        gainControl: { label: 'Source gain', valueDb: 0 },
        icon: 'MIC',
        meterBanks: [{ channels: 2, nodeId: 'resized-source', portId: 'audio' }],
        title: 'Resized source',
      },
    },
  ],
  edges: [],
};

const resizedProviderOptionsFixture: AudioGraphModel = {
  ...resizedOptionsFixture,
  nodes: resizedOptionsFixture.nodes.map((node) => ({
    ...node,
    size: { width: 300, height: 100 },
    runtime: { type: 'provider', config: {} },
    appearance: {
      ...node.appearance,
      category: 'Processor',
      title: 'Resized provider',
    },
  })),
};

const routingFixture: AudioGraphModel = {
  revision: 1,
  nodes: [
    {
      id: 'source-64',
      position: { x: 90, y: 55 },
      inputPorts: [],
      outputPorts: channels('source-64', 'output', 64),
      appearance: {
        category: 'Source',
        channelCount: 64,
        detail: '64-channel source · Options below the channel face',
        gainControl: { label: 'Source gain', valueDb: 0 },
        icon: 'APP',
        meterBanks: [{ channels: 64, nodeId: 'source-64', portId: 'audio' }],
        title: 'Source 64',
      },
    },
    {
      id: 'sink-16',
      position: { x: 700, y: 285 },
      inputPorts: channels('sink-16', 'input', 16),
      outputPorts: [],
      appearance: {
        category: 'Output',
        channelCount: 16,
        detail: '16-channel destination',
        icon: 'IO',
        meterBanks: [{ channels: 16, nodeId: 'sink-16', portId: 'audio' }],
        status: 'Ready',
        title: 'Destination 16',
      },
    },
  ],
  edges: [],
};

const channelSelectionFixture: AudioGraphModel = {
  ...routingFixture,
  nodes: [
    ...routingFixture.nodes,
    {
      id: 'second-source',
      position: { x: 400, y: 55 },
      inputPorts: [],
      outputPorts: channels('second-source', 'output', 8),
      appearance: {
        category: 'Source',
        channelCount: 8,
        icon: 'APP',
        title: 'Second source · not included in the first source selection',
        meterBanks: [{ channels: 8, nodeId: 'second-source', portId: 'audio' }],
      },
    },
  ],
};

const twoApplicationSourcesFixture: AudioGraphModel = {
  revision: 1,
  nodes: [
    ...[
      { id: 'mixrelay-app', title: 'MixRelay', iconUrl: mixrelayAppIcon, y: 40 },
      { id: 'quicktime-app', title: 'QuickTime Player', iconUrl: quickTimeAppIcon, y: 500 },
    ].map(({ id, title, iconUrl, y }) => ({
      id,
      position: { x: 100, y },
      size: { width: 280, height: 320 },
      inputPorts: [],
      outputPorts: channels(id, 'output', 32),
      appearance: {
        category: 'Application',
        channelCount: 32,
        icon: 'APP',
        iconUrl,
        title,
      },
    })),
    {
      id: 'test-output',
      position: { x: 640, y: 270 },
      size: { width: 280, height: 320 },
      inputPorts: channels('test-output', 'input', 32),
      outputPorts: [],
      appearance: {
        category: 'Output',
        channelCount: 32,
        icon: 'IO',
        title: 'Output 32',
      },
    },
  ],
  edges: [],
};

const releaseMenuItems: readonly AudioGraphNodeMenuItem[] = [
  {
    children: Array.from({ length: 36 }, (_, index) => ({
      description: `${index % 2 === 0 ? 64 : 16} channels`,
      id: `source-${index + 1}`,
      label: `Source catalog item ${String(index + 1).padStart(2, '0')}`,
    })),
    id: 'source-catalog',
    label: 'Sources',
  },
  {
    children: Array.from({ length: 36 }, (_, index) => ({
      description: 'Processor',
      id: `processor-${index + 1}`,
      label: `Processor catalog item ${String(index + 1).padStart(2, '0')}`,
    })),
    id: 'processor-catalog',
    label: 'Processors',
  },
  {
    children: Array.from({ length: 36 }, (_, index) => ({
      description: 'Output',
      id: `output-${index + 1}`,
      label: `Output catalog item ${String(index + 1).padStart(2, '0')}`,
    })),
    id: 'output-catalog',
    label: 'Outputs',
  },
];

function mappedChannels(model: AudioGraphModel) {
  return model.edges.map((edge) => {
    const source = model.nodes.find((node) => node.id === edge.sourceNodeId);
    const target = model.nodes.find((node) => node.id === edge.targetNodeId);
    const sourcePort = source?.outputPorts.find((port) => port.id === edge.sourcePortId);
    const targetPort = target?.inputPorts.find((port) => port.id === edge.targetPortId);
    return `${source?.appearance.title ?? edge.sourceNodeId} CH ${(sourcePort?.channelIndex ?? -1) + 1} → ${target?.appearance.title ?? edge.targetNodeId} CH ${(targetPort?.channelIndex ?? -1) + 1}`;
  });
}

function ReleaseAcceptanceGraph() {
  const [model, setModel] = useState(routingFixture);
  const [lastMenuItem, setLastMenuItem] = useState('None');
  const mappings = mappedChannels(model);

  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: 'minmax(0, 1fr) 280px',
        height: '100dvh',
        minHeight: 700,
      }}
    >
      <div style={{ minHeight: 0, minWidth: 0 }}>
        <AudioGraphCanvas
          addNodeLabel="Add node"
          ariaLabel="Release acceptance audio graph"
          model={model}
          nodeMenuItems={releaseMenuItems}
          onAddNode={(itemId) => setLastMenuItem(itemId)}
          onModelChange={setModel}
          onNodeGainChange={(nodeId, valueDb) => {
            setModel((current) => ({
              ...current,
              nodes: current.nodes.map((node) =>
                node.id === nodeId
                  ? {
                      ...node,
                      appearance: {
                        ...node.appearance,
                        gainControl: node.appearance.gainControl
                          ? { ...node.appearance.gainControl, valueDb }
                          : undefined,
                      },
                    }
                  : node,
              ),
            }));
          }}
        />
      </div>
      <aside
        aria-label="Release acceptance fixture"
        style={{
          background: 'var(--surface)',
          borderLeft: '1px solid var(--border-strong)',
          overflow: 'auto',
          padding: 16,
        }}
      >
        <h2 style={{ fontSize: 16, marginTop: 0 }}>Browser acceptance fixture</h2>
        <p>Real AudioGraphCanvas and ReactFlow; changes stay in this local story fixture.</p>
        <p>
          64 output channels → 16 input channels. Box-select outputs on one node; the live summary
          shows a small range/count label beside the pointer only while selecting. Drop on an input
          to compare preview/final mapping.
        </p>
        <p>
          Shift adds outputs from the same node; inputs and other sources are excluded. Esc cancels
          channel selection. Test middle-button and Mod-drag pan separately.
        </p>
        <p>
          Open Source 64 Options and confirm CH anchors stay put. Add node → catalog submenu to test
          bounded scrolling.
        </p>
        <button onClick={() => setModel(routingFixture)} type="button">
          Reset graph fixture
        </button>
        <p>
          Last menu selection: <output aria-label="Last menu selection">{lastMenuItem}</output>
        </p>
        <p>
          Connection count: <output aria-label="Connection count">{model.edges.length}</output>
        </p>
        <ol aria-label="Final channel mapping" style={{ paddingLeft: 20 }}>
          {mappings.map((mapping) => (
            <li key={mapping}>{mapping}</li>
          ))}
        </ol>
      </aside>
    </div>
  );
}
