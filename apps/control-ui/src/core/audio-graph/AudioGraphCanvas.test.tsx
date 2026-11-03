import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ComponentType, CSSProperties, PropsWithChildren } from 'react';
import { useState } from 'react';
import { assert, beforeEach, describe, expect, it, vi } from 'vitest';
import { AudioMeterStore } from '../audio-meter/AudioMeterStore';
import { i18n } from '../i18n';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { frequencyPosition, gainPosition } from '../ui/equalizerResponse';
import { NodeOptionsSection } from '../ui/NodeOptionsSection';
import { useNodeOptionsState } from '../ui/nodeOptionsState';
import { AudioGraphCanvas } from './AudioGraphCanvas';
import type { AudioGraphViewport } from './graphViewport';
import type { AudioGraphModel, AudioGraphNode } from './model';

type MockItem = Record<string, unknown>;

function CanvasTestProvider({ children }: PropsWithChildren) {
  return <MixRelayProvider>{children}</MixRelayProvider>;
}

const viewportHarness = {
  current: { x: 0, y: 0, zoom: 1 } as AudioGraphViewport,
  setViewport: vi.fn(),
  fitView: vi.fn(() => Promise.resolve(true)),
  initialize: null as (() => void) | null,
  lastMoveEnd: null as ((event: Event | null, view: AudioGraphViewport) => void) | null,
};
const updateNodeInternals = vi.fn();
const connectionHarness: MockItem = { inProgress: false, toHandle: null };
const previewHarness = {
  targetAvailable: true,
  enabled: false,
  sourcePortId: 'out' as string | null,
  targetPortId: 'in',
  zoom: 1,
};
const resizeHarness = { height: 480 };
const deleteHarness = {
  onBeforeDelete: null as (() => Promise<boolean>) | null,
  allowed: () => (deleteHarness.onBeforeDelete as () => Promise<boolean>)(),
};

beforeEach(() => {
  connectionHarness.inProgress = false;
  connectionHarness.toHandle = null;
  previewHarness.targetAvailable = true;
  previewHarness.enabled = false;
  previewHarness.sourcePortId = 'out';
  previewHarness.targetPortId = 'in';
  previewHarness.zoom = 1;
});

function MockViewportActions({ props }: { props: Record<string, unknown> }) {
  const moveEnd = props.onMoveEnd as (event: Event | null, view: AudioGraphViewport) => void;
  viewportHarness.lastMoveEnd = moveEnd;
  const pan = { x: 120, y: -30, zoom: 1.25 };
  const zoom = { ...pan, zoom: 1.75 };
  const finish = (event: Event | null, viewport: AudioGraphViewport) => {
    viewportHarness.current = { ...viewport };
    moveEnd(event, viewport);
  };
  const initialize = () => {
    const onInit = props.onInit as (instance: object) => void;
    onInit({
      getNodes: () => props.nodes,
      getViewport: () => viewportHarness.current,
      setViewport: (viewport: AudioGraphViewport, options: unknown) => {
        viewportHarness.setViewport(viewport, options);
        finish(null, viewport);
        return Promise.resolve(true);
      },
      fitView: viewportHarness.fitView,
      screenToFlowPosition: (position: { x: number; y: number }) => position,
    });
  };
  viewportHarness.initialize = initialize;
  return (
    <>
      <button aria-label="initialize viewport" onClick={initialize} type="button" />
      <button
        aria-label="viewport moving"
        onClick={() => {
          const onMove = props.onMove as
            | ((event: Event, view: AudioGraphViewport) => void)
            | undefined;
          onMove?.(new MouseEvent('mousemove'), pan);
        }}
        type="button"
      />
      <button
        aria-label="finish viewport pan"
        onClick={() => finish(new MouseEvent('mouseup'), pan)}
        type="button"
      />
      <button
        aria-label="finish viewport zoom"
        onClick={() => finish(new WheelEvent('wheel'), zoom)}
        type="button"
      />
      <button
        aria-label="finish programmatic viewport"
        onClick={() => finish(null, zoom)}
        type="button"
      />
    </>
  );
}

function MockConnectionActions({
  edges,
  invalidConnection,
  onConnect,
  onReconnect,
  onReconnectEnd,
  validConnection,
}: {
  edges: MockItem[];
  invalidConnection: MockItem;
  onConnect: (connection: MockItem) => void;
  onReconnect: (edge: MockItem, connection: MockItem) => void;
  onReconnectEnd: (
    event: MouseEvent,
    edge: MockItem,
    handleType: string,
    state: { isValid: boolean },
  ) => void;
  validConnection: MockItem;
}) {
  const finishReconnect = (isValid: boolean) =>
    onReconnectEnd(new MouseEvent('pointerup'), edges[0] ?? {}, 'target', { isValid });
  return (
    <>
      <button aria-label="connect" onClick={() => onConnect(validConnection)} type="button" />
      <button
        aria-label="invalid connect"
        onClick={() => onConnect(invalidConnection)}
        type="button"
      />
      <button
        aria-label="reconnect"
        onClick={() => onReconnect(edges[0] ?? {}, validConnection)}
        type="button"
      />
      <button
        aria-label="invalid reconnect"
        onClick={() => onReconnect({ id: 'missing' }, invalidConnection)}
        type="button"
      />
      <button
        aria-label="finish valid reconnect"
        onClick={() => finishReconnect(true)}
        type="button"
      />
      <button
        aria-label="drop reconnect in empty space"
        onClick={() => finishReconnect(false)}
        type="button"
      />
    </>
  );
}

function MockDeletionActions({
  edges,
  nodes,
  onDelete,
  onEdgeDoubleClick,
}: {
  edges: MockItem[];
  nodes: MockItem[];
  onDelete: (selection: { edges: MockItem[]; nodes: MockItem[] }) => void;
  onEdgeDoubleClick: (event: MouseEvent, edge: MockItem) => void;
}) {
  return (
    <>
      <button
        aria-label="delete selected"
        onClick={() => onDelete({ edges, nodes: [] })}
        type="button"
      />
      <button
        aria-label="delete node"
        onClick={() => onDelete({ edges: [], nodes: [nodes[1] ?? {}] })}
        type="button"
      />
      <button
        aria-label="delete double click"
        onClick={() => onEdgeDoubleClick(new MouseEvent('dblclick'), edges[0] ?? {})}
        type="button"
      />
    </>
  );
}

function MockMoveAction({ onNodesChange }: { onNodesChange: (changes: MockItem[]) => void }) {
  return (
    <button
      aria-label="move"
      onClick={() =>
        onNodesChange([
          { id: 'processor', position: { x: 220, y: 140 }, type: 'position' },
          { id: 'source', type: 'select' },
          { id: 'missing', position: { x: 1, y: 1 }, type: 'position' },
        ])
      }
      type="button"
    />
  );
}

function MockMeasurementActions({
  onNodesChange,
}: {
  onNodesChange: (changes: MockItem[]) => void;
}) {
  return (
    <>
      <button
        type="button"
        aria-label="measure source"
        onClick={() =>
          onNodesChange([
            { id: 'source', type: 'dimensions', dimensions: { width: 320, height: 250 } },
          ])
        }
      />
      <button
        type="button"
        aria-label="resize sink"
        onClick={() =>
          onNodesChange([
            {
              id: 'sink',
              type: 'dimensions',
              dimensions: { width: 320, height: 250 },
              setAttributes: true,
            },
          ])
        }
      />
    </>
  );
}

function applyMockNodeDimensions(node: MockItem, changes: MockItem[]) {
  const change = changes.find(
    (candidate) => candidate.type === 'dimensions' && candidate.id === node.id,
  );
  if (!change?.dimensions) return node;
  const dimensions = change.dimensions as { width: number; height: number };
  return {
    ...node,
    measured: dimensions,
    ...(change.setAttributes === true || change.setAttributes === 'width'
      ? { width: dimensions.width }
      : {}),
    ...(change.setAttributes === true || change.setAttributes === 'height'
      ? { height: dimensions.height }
      : {}),
  };
}

function useMockFlowState(initial: unknown) {
  const [state, setState] = useState(initial);
  const applyChanges = (changes: MockItem[]) =>
    setState((current: unknown) =>
      Array.isArray(current)
        ? (current as MockItem[]).map((node) => applyMockNodeDimensions(node, changes))
        : current,
    );
  return [state, setState, applyChanges];
}

function mockConnectionNode(id: string, side: 'source' | 'target', pitch: number) {
  return {
    id,
    internals: {
      handleBounds: {
        [side]: Array.from({ length: 4 }, (_, index) => ({
          id: `${side === 'source' ? 'out' : 'in'}-${index + 1}`,
          y: index * pitch,
          height: 8,
        })),
      },
    },
  };
}

function MockRenameBoundary({
  props,
  nodes,
}: {
  props: Record<string, unknown>;
  nodes: MockItem[];
}) {
  return (
    <button
      type="button"
      aria-label="non-element node double-click"
      onClick={() => {
        const callback = props.onNodeDoubleClick as
          | ((event: unknown, node: MockItem) => void)
          | undefined;
        const node = nodes[0];
        if (node) callback?.({ target: document.createTextNode('Source') }, node);
      }}
    />
  );
}

function MockReactFlow({ children, ...props }: PropsWithChildren<Record<string, unknown>>) {
  const nodes = props.nodes as MockItem[];
  const edges = props.edges as MockItem[];
  const nodeTypes = props.nodeTypes as Record<string, ComponentType<MockItem>>;
  const AudioNode = nodeTypes.audio as ComponentType<MockItem>;
  deleteHarness.onBeforeDelete = props.onBeforeDelete as () => Promise<boolean>;
  const validConnection = {
    source: 'processor',
    sourceHandle: previewHarness.sourcePortId,
    target: 'sink',
    targetHandle: previewHarness.targetPortId,
  };
  const invalidConnection = {
    source: 'source',
    sourceHandle: null,
    target: 'processor',
    targetHandle: null,
  };
  const isValidConnection = props.isValidConnection as (connection: MockItem) => boolean;
  return (
    <div
      data-selected-nodes={nodes
        .filter((node) => node.selected)
        .map((node) => node.id)
        .join(',')}
      data-selected-edges={edges
        .filter((edge) => edge.selected)
        .map((edge) => edge.id)
        .join(',')}
      data-connection-valid={isValidConnection(validConnection)}
      data-reconnect-valid={isValidConnection(edges[0] ?? {})}
      data-fit-view={props.fitView as boolean}
      data-min-zoom={props.minZoom as number}
      data-max-zoom={props.maxZoom as number}
      data-default-viewport={JSON.stringify(props.defaultViewport)}
      data-pan-activation-key={props.panActivationKeyCode as string}
      data-testid="react-flow"
    >
      <MockAriaLabels config={props.ariaLabelConfig as MockItem} edges={edges} />
      {nodes.map((node) => (
        // biome-ignore lint/a11y/useSemanticElements: Match React Flow's focusable node group.
        <div
          className="react-flow__node"
          role="group"
          aria-label={node.ariaLabel as string}
          // biome-ignore lint/a11y/noNoninteractiveTabindex: Mirror the real graph keyboard owner.
          tabIndex={0}
          data-id={node.id as string}
          data-flow-height={node.height as number | undefined}
          data-flow-measured-height={
            (node.measured as MockItem | undefined)?.height as number | undefined
          }
          key={node.id as string}
          onDoubleClick={(event) =>
            (props.onNodeDoubleClick as ((event: unknown, node: MockItem) => void) | undefined)?.(
              event,
              node,
            )
          }
        >
          <AudioNode data={node.data} selected={Boolean(node.selected)} />
        </div>
      ))}
      <MockRenameBoundary props={props} nodes={nodes} />
      <MockConnectionActions
        edges={edges}
        invalidConnection={invalidConnection}
        onConnect={props.onConnect as (connection: MockItem) => void}
        onReconnect={props.onReconnect as (edge: MockItem, connection: MockItem) => void}
        onReconnectEnd={props.onReconnectEnd as never}
        validConnection={validConnection}
      />
      <MockDeletionActions
        edges={edges}
        nodes={nodes}
        onDelete={props.onDelete as never}
        onEdgeDoubleClick={props.onEdgeDoubleClick as never}
      />
      <MockMoveAction onNodesChange={props.onNodesChange as never} />
      <MockMeasurementActions onNodesChange={props.onNodesChange as never} />
      <button
        aria-label="resize source"
        onClick={() =>
          (props.onNodesChange as (changes: MockItem[]) => void)([
            {
              id: 'source',
              type: 'dimensions',
              dimensions: { width: 300, height: resizeHarness.height },
              setAttributes: true,
            },
          ])
        }
        type="button"
      />
      <MockViewportActions props={props} />
      <MockConnectionPreview props={props} />
      <button className="react-flow__pane" type="button" onClick={props.onPaneClick as () => void}>
        Graph background
      </button>
      {children}
    </div>
  );
}

function MockConnectionPreview({ props }: { props: Record<string, unknown> }) {
  return (
    <>
      {previewHarness.enabled && (
        <svg aria-label="Connection preview">
          {(() => {
            const ConnectionLine = props.connectionLineComponent as ComponentType<MockItem>;
            return (
              <ConnectionLine
                fromHandle={{ id: previewHarness.sourcePortId }}
                fromNode={mockConnectionNode('processor', 'source', 20)}
                fromPosition="right"
                fromX={10}
                fromY={20}
                toHandle={
                  previewHarness.targetAvailable ? { id: previewHarness.targetPortId } : undefined
                }
                toNode={
                  previewHarness.targetAvailable
                    ? mockConnectionNode('sink', 'target', 26)
                    : undefined
                }
                toPosition="left"
                toX={100}
                toY={40}
              />
            );
          })()}
        </svg>
      )}
    </>
  );
}

vi.mock('@xyflow/react', () => ({
  Background: () => <span data-testid="background" />,
  Panel: ({ children }: PropsWithChildren) => <div>{children}</div>,
  BackgroundVariant: { Lines: 'lines' },
  ConnectionMode: { Strict: 'strict' },
  getBezierPath: ({ sourceX, sourceY, targetX, targetY }: Record<string, number>) => [
    `M${sourceX},${sourceY}L${targetX},${targetY}`,
  ],
  Handle: ({ id, type, position: _position, ...props }: Record<string, unknown>) => (
    <button
      {...props}
      data-handle-id={id as string}
      data-handle-type={type as string}
      type="button"
    />
  ),
  Position: { Left: 'left', Right: 'right' },
  NodeResizeControl: ({
    position,
    style,
    minHeight,
    minWidth,
  }: {
    position: string;
    style: CSSProperties;
    minHeight: number;
    minWidth: number;
  }) => (
    <span
      data-resize-grip={position}
      data-min-height={minHeight}
      data-min-width={minWidth}
      style={style}
    />
  ),
  ReactFlow: MockReactFlow,
  useConnection: (selector: (connection: MockItem) => unknown) => selector(connectionHarness),
  useEdgesState: useMockFlowState,
  useNodesState: useMockFlowState,
  useUpdateNodeInternals: () => updateNodeInternals,
  useViewport: () => ({ zoom: previewHarness.zoom }),
}));

function MockAriaLabels({ config, edges }: { config: MockItem; edges: MockItem[] }) {
  const moved = config['node.a11yDescription.ariaLiveMessage'] as (value: MockItem) => string;
  return (
    <div data-testid="flow-aria">
      <span data-testid="node-description">
        {config['node.a11yDescription.keyboardDisabled'] as string}
      </span>
      <span data-testid="edge-description">{config['edge.a11yDescription.default'] as string}</span>
      <span data-testid="node-moved">{moved({ direction: 'left', x: 12, y: 34 })}</span>
      {edges.map((edge) => (
        <span data-testid="edge-label" key={edge.id as string}>
          {edge.ariaLabel as string}
        </span>
      ))}
    </div>
  );
}

function model(): AudioGraphModel {
  return {
    revision: 1,
    nodes: [
      {
        id: 'source',
        position: { x: 0, y: 0 },
        inputPorts: [],
        outputPorts: [{ id: 'out', label: 'Source output' }],
        appearance: { category: 'Source', detail: 'App', icon: 'S', title: 'Source' },
      },
      {
        id: 'processor',
        position: { x: 100, y: 0 },
        inputPorts: [{ id: 'in', label: 'Processor input' }],
        outputPorts: [{ id: 'out', label: 'Processor output' }],
        appearance: { category: 'Processor', detail: 'Gain', icon: 'P', title: 'Processor' },
      },
      {
        id: 'sink',
        position: { x: 300, y: 0 },
        inputPorts: [{ id: 'in', label: 'Sink input' }],
        outputPorts: [],
        appearance: { category: 'Output', detail: 'OBS', icon: 'O', title: 'Sink' },
      },
    ],
    edges: [
      {
        id: 'source-to-processor',
        sourceNodeId: 'source',
        sourcePortId: 'out',
        targetNodeId: 'processor',
        targetPortId: 'in',
      },
    ],
  };
}

it('keeps Options and the same gain slider mounted across external control revisions', async () => {
  const initial = model();
  const source = initial.nodes[0];
  assert(source);
  source.appearance.gainControl = { label: 'Input gain', valueDb: 0 };
  const props = { ariaLabel: 'Graph', onModelChange: vi.fn(), onNodeGainChange: vi.fn() };
  const view = render(<AudioGraphCanvas {...props} model={initial} />, {
    wrapper: CanvasTestProvider,
  });
  const options = screen.getByRole('button', { name: (name) => name.endsWith('Options') });
  fireEvent.click(options);
  const slider = screen.getByRole('slider', { name: 'Input gain' });
  let revision = initial.revision;
  for (const valueDb of [-0.5, 0, -1, 0]) {
    revision += 1;
    view.rerender(
      <AudioGraphCanvas
        {...props}
        model={{
          ...initial,
          revision,
          nodes: initial.nodes.map((node) => ({
            ...node,
            appearance: {
              ...node.appearance,
              ...(node.id === 'source' ? { gainControl: { label: 'Input gain', valueDb } } : {}),
            },
          })),
        }}
      />,
    );
    await waitFor(() => expect(slider).toHaveValue(String(valueDb)));
    expect(screen.getByRole('slider', { name: 'Input gain' })).toBe(slider);
    expect(options).toHaveAttribute('aria-expanded', 'true');
  }
  fireEvent.click(screen.getByRole('button', { name: 'Graph background' }));
  expect(options).toHaveAttribute('aria-expanded', 'false');
});

function canvas(viewport?: AudioGraphViewport, onViewportChange = vi.fn()) {
  return (
    <CanvasTestProvider>
      <AudioGraphCanvas
        ariaLabel="Routing graph"
        model={model()}
        onModelChange={vi.fn()}
        onViewportChange={onViewportChange}
        viewport={viewport}
      />
    </CanvasTestProvider>
  );
}

it.each([
  ['MacIntel', 'Meta'],
  ['Linux x86_64', 'Control'],
])('uses the displayed platform modifier for canvas panning on %s', (platform, modifier) => {
  const identity = vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
  try {
    render(canvas());
    expect(screen.getByTestId('react-flow')).toHaveAttribute('data-pan-activation-key', modifier);
  } finally {
    identity.mockRestore();
  }
});

it.each(['input', 'output'] as const)(
  'shows the exact drag-target channel on the %s side and preserves anchors through Options',
  async (side) => {
    const graph = model();
    const processor = graph.nodes[1];
    assert(processor);
    processor.inputPorts = Array.from({ length: 64 }, (_, index) => ({
      id: `in-${index}`,
      label: `Input ${index + 1}`,
      channelIndex: index,
    }));
    processor.outputPorts = Array.from({ length: 64 }, (_, index) => ({
      id: `out-${index}`,
      label: `Output ${index + 1}`,
      channelIndex: index,
    }));
    processor.appearance.gainControl = { label: 'Input gain', valueDb: 0 };
    connectionHarness.inProgress = true;
    connectionHarness.toHandle = {
      nodeId: 'processor',
      id: `${side === 'input' ? 'in' : 'out'}-63`,
    };
    const view = render(
      <AudioGraphCanvas
        ariaLabel="Graph"
        model={graph}
        onModelChange={vi.fn()}
        onNodeGainChange={vi.fn()}
      />,
      { wrapper: CanvasTestProvider },
    );
    expect(await screen.findByRole('tooltip')).toHaveTextContent('CH 64');
    expect(view.container.querySelectorAll('[data-connection-target]')).toHaveLength(1);
    const handles = Array.from(
      view.container.querySelectorAll<HTMLElement>('[data-audio-node-id="processor"]'),
    );
    expect(handles).toHaveLength(128);
    const anchors = handles.map((handle) => handle.style.top);
    const options = screen.getByRole('button', { name: (name) => name.endsWith('Options') });
    fireEvent.click(options);
    expect(screen.getByRole('slider', { name: 'Input gain' })).toBeVisible();
    expect(handles.map((handle) => handle.style.top)).toEqual(anchors);
    fireEvent.click(options);
    expect(handles.map((handle) => handle.style.top)).toEqual(anchors);
  },
);

it('renders and updates separate source and sink channel mini bars', async () => {
  const graph = model();
  const source = graph.nodes[0];
  const sink = graph.nodes[2];
  assert(source && sink);
  source.appearance.meterBanks = [{ channels: 2, nodeId: 'source', portId: 'from_os' }];
  sink.appearance.meterBanks = [{ channels: 2, nodeId: 'sink', portId: 'to_os' }];
  const store = new AudioMeterStore();
  store.update({
    nodes: [
      { nodeId: 'source', portId: 'from_os', peaks: [1, 0] },
      { nodeId: 'sink', portId: 'to_os', peaks: [0, 1] },
    ],
    protocolVersion: 1,
    revision: graph.revision,
  });
  const view = render(
    <AudioGraphCanvas ariaLabel="Graph" meterStore={store} model={graph} onModelChange={vi.fn()} />,
    { wrapper: CanvasTestProvider },
  );
  const sourceBank = view.container.querySelector('[data-id="source"] [data-meter-port="from_os"]');
  const sinkBank = view.container.querySelector('[data-id="sink"] [data-meter-port="to_os"]');
  expect(sourceBank?.querySelectorAll('[data-orientation="horizontal"]')).toHaveLength(2);
  expect(sinkBank?.querySelectorAll('[data-orientation="horizontal"]')).toHaveLength(2);
  expect(sourceBank?.querySelector('[title="Source channel 1: 100%"]')).not.toBeNull();
  expect(sinkBank?.querySelector('[title="Sink channel 2: 100%"]')).not.toBeNull();

  store.update({
    nodes: [
      { nodeId: 'source', portId: 'from_os', peaks: [0, 1] },
      { nodeId: 'sink', portId: 'to_os', peaks: [1, 0] },
    ],
    protocolVersion: 1,
    revision: graph.revision,
  });
  await waitFor(() => {
    expect(sourceBank?.querySelector('[title="Source channel 2: 100%"]')).not.toBeNull();
    expect(sinkBank?.querySelector('[title="Sink channel 1: 100%"]')).not.toBeNull();
  });
});

function selectPortRectangle(
  start: { x: number; y: number },
  end: { x: number; y: number },
  additive = false,
) {
  for (const [type, point] of [
    ['pointerdown', start],
    ['pointerup', end],
  ] as const) {
    portPointer(type, point, additive);
  }
}

function portPointer(
  type: string,
  point: { x: number; y: number },
  additive = false,
  pointerId = 1,
) {
  fireEvent(
    screen.getByRole('button', { name: 'Graph background' }),
    Object.assign(
      new MouseEvent(type, {
        bubbles: true,
        button: 0,
        clientX: point.x,
        clientY: point.y,
        shiftKey: additive,
      }),
      { pointerId, pointerType: 'mouse', isPrimary: true },
    ),
  );
}

it('maps 64 selected outputs to destination capacity without box-selecting inputs', () => {
  const graph = model();
  const processor = graph.nodes[1];
  const sink = graph.nodes[2];
  assert(processor && sink);
  processor.inputPorts = [];
  processor.outputPorts = Array.from({ length: 64 }, (_, index) => ({
    id: index === 0 ? 'out' : `out-${index}`,
    label: `Output ${index + 1}`,
  }));
  sink.inputPorts = Array.from({ length: 16 }, (_, index) => ({
    id: index === 0 ? 'in' : `in-${index}`,
    label: `Input ${index + 1}`,
  }));
  graph.nodes = [processor, sink];
  graph.edges = [];
  const onChange = vi.fn();
  const view = render(
    <AudioGraphCanvas ariaLabel="Graph" model={graph} onModelChange={onChange} />,
    { wrapper: CanvasTestProvider },
  );
  const handles = view.container.querySelectorAll<HTMLElement>('[data-audio-port-side]');
  for (const [index, handle] of handles.entries()) {
    const input = handle.dataset.audioPortSide === 'input';
    vi.spyOn(handle, 'getBoundingClientRect').mockReturnValue(
      new DOMRect(input ? 100 : 10, (input ? index - 64 : index) * 3 + 1, 2, 2),
    );
  }
  selectPortRectangle({ x: 0, y: 0 }, { x: 20, y: 200 });
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(64);
  selectPortRectangle({ x: 90, y: 0 }, { x: 110, y: 50 }, true);
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(64);
  fireEvent.click(screen.getByRole('button', { name: 'connect' }));
  expect(onChange).toHaveBeenCalledOnce();
  const [changed, kind] = onChange.mock.calls[0] as [AudioGraphModel, string];
  expect(kind).toBe('runtime');
  expect(changed.edges).toHaveLength(16);
  expect(changed.edges.map((edge) => [edge.sourcePortId, edge.targetPortId])).toEqual(
    Array.from({ length: 16 }, (_, index) => [
      index === 0 ? 'out' : `out-${index}`,
      index === 0 ? 'in' : `in-${index}`,
    ]),
  );
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(0);
});

it('previews the released target even when a rectangle covers several unselectable inputs', () => {
  const graph = model();
  const sink = graph.nodes[2];
  assert(sink);
  sink.inputPorts.push({ id: 'in-2', label: 'Sink input 2' });
  previewHarness.enabled = true;
  previewHarness.targetPortId = 'in-2';
  const onChange = vi.fn();
  const props = { ariaLabel: 'Graph', model: graph, onModelChange: onChange };
  const view = render(<AudioGraphCanvas {...props} />, { wrapper: CanvasTestProvider });
  const sinkInputs = view.container.querySelectorAll<HTMLElement>(
    '[data-audio-node-id="sink"][data-audio-port-side="input"]',
  );
  expect(sinkInputs).toHaveLength(2);
  sinkInputs.forEach((handle, index) => {
    vi.spyOn(handle, 'getBoundingClientRect').mockReturnValue(
      new DOMRect(100, 10 + index * 20, 2, 2),
    );
  });
  selectPortRectangle({ x: 90, y: 0 }, { x: 110, y: 50 });
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(0);
  view.rerender(<AudioGraphCanvas {...props} />);
  expect(view.container.querySelectorAll('.react-flow__connection-path')).toHaveLength(1);
  expect(view.container.querySelector('.react-flow__connection-path')).toHaveAttribute(
    'd',
    'M10,20L100,40',
  );
  fireEvent.click(screen.getByRole('button', { name: 'connect' }));
  expect(onChange).toHaveBeenCalledOnce();
  const [changed] = onChange.mock.calls[0] as [AudioGraphModel];
  expect(changed.edges.at(-1)).toMatchObject({ sourcePortId: 'out', targetPortId: 'in-2' });
});

it.each([true, false])(
  'keeps the dragged source and target paired with selected target group %s',
  (selectTargets) => {
    const graph = model();
    const processor = graph.nodes[1];
    const sink = graph.nodes[2];
    assert(processor && sink);
    processor.inputPorts = [];
    processor.outputPorts = Array.from({ length: 4 }, (_, index) => ({
      id: `out-${index + 1}`,
      label: `Output ${index + 1}`,
    }));
    sink.inputPorts = Array.from({ length: 4 }, (_, index) => ({
      id: `in-${index + 1}`,
      label: `Input ${index + 1}`,
    }));
    graph.nodes = [processor, sink];
    graph.edges = [];
    previewHarness.enabled = true;
    previewHarness.sourcePortId = 'out-3';
    previewHarness.targetPortId = 'in-2';
    previewHarness.zoom = 0.6;
    const onChange = vi.fn();
    const props = { ariaLabel: 'Graph', model: graph, onModelChange: onChange };
    const view = render(<AudioGraphCanvas {...props} />, { wrapper: CanvasTestProvider });
    const handles = view.container.querySelectorAll<HTMLElement>('[data-audio-port-side]');
    for (const handle of handles) {
      const portIndex = Number(handle.dataset.audioPortId?.split('-')[1]) - 1;
      const input = handle.dataset.audioPortSide === 'input';
      vi.spyOn(handle, 'getBoundingClientRect').mockReturnValue(
        new DOMRect(input ? 100 : 10, 10 + portIndex * 20, 2, 2),
      );
    }
    selectPortRectangle({ x: 0, y: 0 }, { x: 20, y: 80 });
    if (selectTargets) selectPortRectangle({ x: 90, y: 0 }, { x: 110, y: 80 }, true);
    expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(4);
    view.rerender(<AudioGraphCanvas {...props} />);
    expect(view.container.querySelectorAll('.react-flow__connection-path')).toHaveLength(3);
    expect(
      Array.from(view.container.querySelectorAll('.react-flow__connection-path')).map((path) =>
        path.getAttribute('d'),
      ),
    ).toContain('M10,20L100,40');
    expect(
      Array.from(view.container.querySelectorAll('.react-flow__connection-path')).map((path) =>
        path.getAttribute('d'),
      ),
    ).toEqual(['M10,0L100,14', 'M10,20L100,40', 'M10,40L100,66']);
    fireEvent.click(screen.getByRole('button', { name: 'connect' }));
    expect(onChange).toHaveBeenCalledOnce();
    const [changed] = onChange.mock.calls[0] as [AudioGraphModel];
    expect(changed.edges.map((edge) => [edge.sourcePortId, edge.targetPortId])).toEqual([
      ['out-2', 'in-1'],
      ['out-3', 'in-2'],
      ['out-4', 'in-3'],
    ]);
  },
);

function resetViewportHarness() {
  viewportHarness.current = { x: 0, y: 0, zoom: 1 };
  viewportHarness.setViewport.mockClear();
  viewportHarness.lastMoveEnd = null;
}

it('resets the resize grip translation independently of stylesheet transforms', () => {
  const { container } = render(canvas());
  const grips = container.querySelectorAll('[data-resize-grip="bottom-right"]');
  expect(grips).toHaveLength(3);
  for (const grip of grips) expect(grip).toHaveStyle({ translate: 'none' });
});

function installMeasuredResizeObserver() {
  const originalObserver = globalThis.ResizeObserver;
  const observers = new Map<Element, ResizeObserverCallback>();
  globalThis.ResizeObserver = class implements ResizeObserver {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe(target: Element) {
      observers.set(target, this.callback);
    }
    unobserve(target: Element) {
      observers.delete(target);
    }
    disconnect() {}
  };
  return {
    restore: () => {
      globalThis.ResizeObserver = originalObserver;
    },
    reportHeight: (target: Element, height: number) => {
      const style = (target as HTMLElement).style;
      style.boxSizing = 'border-box';
      style.height = `${height}px`;
      const callback = observers.get(target);
      assert(callback);
      act(() =>
        callback(
          [
            {
              target,
              borderBoxSize: [{ inlineSize: 300, blockSize: height }],
              contentRect: { x: 0, y: 0, top: 0, left: 0, right: 300, bottom: height },
            } as unknown as ResizeObserverEntry,
          ],
          {} as ResizeObserver,
        ),
      );
    },
  };
}

function ControlledResizeCanvas({
  initial,
  committed,
}: {
  initial: AudioGraphModel;
  committed: (model: AudioGraphModel, kind: string) => void;
}) {
  const [graph, setGraph] = useState(initial);
  return (
    <AudioGraphCanvas
      ariaLabel="Routing graph"
      model={graph}
      onModelChange={(next, kind) => {
        committed(next, kind);
        setGraph(next);
      }}
      onNodeGainChange={vi.fn()}
    />
  );
}

function resizedOptionsModel() {
  const graph = model();
  const source = graph.nodes[0];
  assert(source);
  source.size = { width: 300, height: 400 };
  source.appearance.gainControl = { label: 'Input gain', valueDb: 0 };
  source.outputPorts = [
    { id: 'left', label: 'Left', channelIndex: 0 },
    { id: 'right', label: 'Right', channelIndex: 1 },
  ];
  return graph;
}

function resizedNodeElements(container: HTMLElement) {
  const flowNode = container.querySelector<HTMLElement>('.react-flow__node[data-id="source"]');
  assert(flowNode);
  const card = flowNode.querySelector<HTMLElement>('[data-face-mode="standard"]');
  assert(card);
  const options = within(flowNode).getByRole('button', {
    name: (name) => name.endsWith('Options'),
  });
  const controls = flowNode.querySelector<HTMLElement>('[data-node-controls]');
  assert(controls);
  const inner = flowNode.querySelector<HTMLElement>('[style*="min-height"]');
  const grip = flowNode.querySelector('[data-resize-grip="bottom-right"]');
  const port = flowNode.querySelector<HTMLElement>('[data-audio-port-id="left"]');
  assert(inner && grip && port);
  return { flowNode, card, options, controls, inner, grip, port };
}

it('keeps stored size and port anchors stable through Options and normalizes open resizes once', async () => {
  const { reportHeight, restore } = installMeasuredResizeObserver();
  try {
    const initial = resizedOptionsModel();
    const committed = vi.fn();
    const view = render(<ControlledResizeCanvas initial={initial} committed={committed} />, {
      wrapper: CanvasTestProvider,
    });
    const { flowNode, card, options, controls, inner, grip, port } = resizedNodeElements(
      view.container,
    );
    reportHeight(controls, 25);
    await waitFor(() =>
      expect(card.style.getPropertyValue('--audio-node-face-height')).toBe('373px'),
    );
    const naturalMinHeight = Number.parseFloat(inner.style.minHeight);
    expect(naturalMinHeight).toBeLessThan(400);
    expect(inner).toHaveStyle({ height: '400px' });
    expect(grip).toHaveAttribute('data-min-height', String(naturalMinHeight));
    const closedTop = port.style.top;

    fireEvent.click(options);
    const panel = flowNode.querySelector('[data-node-disclosure]');
    assert(panel);
    reportHeight(panel, 37.5);
    reportHeight(controls, 62.5);
    await waitFor(() => expect(inner).toHaveStyle({ height: '437.5px' }));
    expect(inner).toHaveStyle({ minHeight: `${naturalMinHeight + 37.5}px` });
    expect(grip).toHaveAttribute('data-min-height', String(naturalMinHeight + 37.5));
    expect((port as HTMLElement).style.top).toBe(closedTop);
    expect(flowNode).not.toHaveAttribute('data-flow-height');
    expect(committed).not.toHaveBeenCalled();

    resizeHarness.height = 480.5;
    fireEvent.click(screen.getByRole('button', { name: 'resize source' }));
    await waitFor(() => {
      const latest = committed.mock.lastCall?.[0] as AudioGraphModel | undefined;
      expect(latest?.nodes[0]?.size?.height).toBe(443);
      expect(inner).toHaveStyle({ height: '480.5px' });
    });
    expect(flowNode).not.toHaveAttribute('data-flow-height');
    expect(flowNode).toHaveAttribute('data-flow-measured-height', '480.5');
    expect((port as HTMLElement).style.top).not.toBe(closedTop);

    fireEvent.click(options);
    reportHeight(controls, 25);
    await waitFor(() => expect(inner).toHaveStyle({ height: '443px' }));
    await waitFor(() =>
      expect((card as HTMLElement).style.getPropertyValue('--audio-node-face-height')).toBe(
        '416px',
      ),
    );
    expect(committed).toHaveBeenCalledTimes(1);
    const resizedTop = (port as HTMLElement).style.top;
    fireEvent.click(options);
    const reopenedPanel = flowNode.querySelector('[data-node-disclosure]');
    assert(reopenedPanel);
    reportHeight(reopenedPanel, 37.5);
    reportHeight(controls, 62.5);
    await waitFor(() => expect(inner).toHaveStyle({ height: '480.5px' }));
    expect(committed).toHaveBeenCalledTimes(1);
    expect((port as HTMLElement).style.top).toBe(resizedTop);
    fireEvent.click(screen.getByRole('button', { name: 'resize source' }));
    expect(
      (committed.mock.lastCall?.[0] as AudioGraphModel | undefined)?.nodes[0]?.size?.height,
    ).toBe(443);
    expect(flowNode).not.toHaveAttribute('data-flow-height');
    expect(flowNode).toHaveAttribute('data-flow-measured-height', '480.5');

    resizeHarness.height = 450.5;
    fireEvent.click(screen.getByRole('button', { name: 'resize source' }));
    await waitFor(() => expect(inner).toHaveStyle({ height: '450.5px' }));
    expect(flowNode).not.toHaveAttribute('data-flow-height');
    expect(flowNode).toHaveAttribute('data-flow-measured-height', '450.5');
    expect(
      (committed.mock.lastCall?.[0] as AudioGraphModel | undefined)?.nodes[0]?.size?.height,
    ).toBe(413);
    fireEvent.click(options);
    reportHeight(controls, 25);
    await waitFor(() => expect(inner).toHaveStyle({ height: '413px' }));
  } finally {
    restore();
  }
});

function RecorderDisclosureFixture() {
  const { opened, toggle } = useNodeOptionsState();
  return (
    <>
      <span>Transport</span>
      <NodeOptionsSection label="Options" opened={opened} onToggle={toggle}>
        <div style={{ height: 400 }}>Recording settings</div>
      </NodeOptionsSection>
    </>
  );
}

it('shares one open Options panel across regular and recorder nodes and closes on the pane', () => {
  const graph = model();
  graph.nodes = graph.nodes.map((node) =>
    node.id === 'source'
      ? { ...node, appearance: { ...node.appearance, gainControl: { label: 'Level', valueDb: 0 } } }
      : node,
  );
  render(
    <CanvasTestProvider>
      <AudioGraphCanvas
        ariaLabel="Routing graph"
        model={graph}
        onModelChange={vi.fn()}
        onNodeGainChange={vi.fn()}
        renderNodeContent={(node) => (node.id === 'sink' ? <RecorderDisclosureFixture /> : null)}
      />
    </CanvasTestProvider>,
  );
  const [gain, recorder] = screen.getAllByRole('button', {
    name: (name) => name.endsWith('Options'),
  });
  assert(gain && recorder);
  fireEvent.click(gain);
  expect(gain).toHaveAttribute('aria-expanded', 'true');
  fireEvent.click(recorder);
  expect(gain).toHaveAttribute('aria-expanded', 'false');
  expect(recorder).toHaveAttribute('aria-expanded', 'true');
  fireEvent.click(gain);
  expect(recorder).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(screen.getByRole('button', { name: 'Graph background' }));
  expect(gain).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(recorder);
  fireEvent.click(screen.getByRole('button', { name: 'Graph background' }));
  expect(recorder).toHaveAttribute('aria-expanded', 'false');
});

function recorderCanvas(channels: number) {
  const graph: AudioGraphModel = {
    revision: channels,
    edges: [],
    nodes: [
      {
        id: 'take',
        position: { x: 0, y: 0 },
        runtime: { type: 'recorder', config: { channels } },
        appearance: {
          title: 'Recorder',
          category: 'Output',
          icon: 'REC',
          meterBanks: [{ channels, nodeId: 'take', portId: 'audio' }],
          channelCount: channels,
        },
        inputPorts: Array.from({ length: channels }, (_, channelIndex) => ({
          id: `audio:${channelIndex}`,
          label: 'Recorder input',
          channelIndex,
        })),
        outputPorts: [],
      },
    ],
  };
  return (
    <CanvasTestProvider>
      <AudioGraphCanvas
        ariaLabel="Recorder graph"
        model={graph}
        onModelChange={vi.fn()}
        renderNodeContent={() => <RecorderDisclosureFixture />}
      />
    </CanvasTestProvider>
  );
}

it('aligns recorder handles with content-sized meter rows through channel edits and disclosure changes', () => {
  const view = render(recorderCanvas(2));
  const face = screen.getByRole('group', { name: 'Recorder' }).querySelector('[data-node-face]');
  const root = face?.closest('[data-face-mode]');
  expect(root?.parentElement).toHaveAttribute('data-node-type', 'recorder');
  expect(root?.parentElement?.style.width).toBe('');
  expect(root).toHaveStyle({ '--audio-node-face-height': '94px' });
  expect(screen.getByLabelText('Recorder input, CH 1')).toHaveStyle({ top: '56.5px' });
  expect(screen.getByLabelText('Recorder input, CH 2')).toHaveStyle({ top: '69.5px' });
  expect(screen.queryByText('2 CH')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Options') }));
  expect(screen.getByText('Recording settings')).toBeInTheDocument();
  expect(root).toHaveStyle({ '--audio-node-face-height': '94px' });
  expect(screen.getByLabelText('Recorder input, CH 2')).toHaveStyle({ top: '69.5px' });
  for (const channels of [8, 1, 2]) {
    updateNodeInternals.mockClear();
    view.rerender(recorderCanvas(channels));
    expect(root).toHaveStyle({
      '--audio-node-face-height': `${Math.max(94, 41 + channels * 13)}px`,
    });
    for (let index = 0; index < channels; index += 1)
      expect(screen.getByLabelText(`Recorder input, CH ${index + 1}`)).toHaveStyle({
        top: `${43 + Math.max(0, 94 - (41 + channels * 13)) / 2 + index * 13}px`,
      });
    expect(screen.queryByLabelText(`Recorder input, CH ${channels + 1}`)).not.toBeInTheDocument();
    expect(updateNodeInternals).toHaveBeenCalledWith('take');
  }
});

describe('AudioGraphCanvas viewport contract', () => {
  beforeEach(resetViewportHarness);

  it('keeps automatic fit for callers without a saved viewport', () => {
    render(canvas());
    expect(screen.getByTestId('react-flow')).toHaveAttribute('data-fit-view', 'true');
    expect(screen.getByTestId('react-flow')).not.toHaveAttribute('data-default-viewport');
    fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
    expect(viewportHarness.setViewport).not.toHaveBeenCalled();
  });

  it('restores a saved viewport without fit or an autosave echo of the restore', () => {
    const viewport = { x: -180, y: 70, zoom: 0.75 };
    const onViewportChange = vi.fn();
    render(canvas(viewport, onViewportChange));
    expect(screen.getByTestId('react-flow')).toHaveAttribute('data-fit-view', 'false');
    expect(screen.getByTestId('react-flow')).toHaveAttribute(
      'data-default-viewport',
      JSON.stringify(viewport),
    );
    fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
    expect(viewportHarness.setViewport).toHaveBeenCalledWith(viewport, { duration: 0 });
    expect(onViewportChange).not.toHaveBeenCalled();
  });

  it('uses the latest external value when initialization finishes late', () => {
    const onViewportChange = vi.fn();
    const view = render(canvas({ x: 20, y: 40, zoom: 1 }, onViewportChange));
    const latest = { x: -60, y: -80, zoom: 1.5 };
    view.rerender(canvas(latest, onViewportChange));
    fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
    expect(viewportHarness.setViewport).toHaveBeenCalledExactlyOnceWith(latest, { duration: 0 });
    expect(onViewportChange).not.toHaveBeenCalled();
  });

  it('restores external changes but ignores equal coordinates on unrelated renders', () => {
    const onViewportChange = vi.fn();
    const view = render(canvas(undefined, onViewportChange));
    fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
    const restored = { x: 200, y: 300, zoom: 0.8 };
    view.rerender(canvas(restored, onViewportChange));
    view.rerender(canvas({ ...restored }, onViewportChange));
    expect(viewportHarness.setViewport).toHaveBeenCalledExactlyOnceWith(restored, { duration: 0 });
    expect(screen.getByTestId('react-flow')).toHaveAttribute('data-fit-view', 'false');
    expect(onViewportChange).not.toHaveBeenCalled();
  });

  it('reports user pan and zoom only when each gesture ends, without graph mutations', () => {
    const onViewportChange = vi.fn();
    const onModelChange = vi.fn();
    render(
      <CanvasTestProvider>
        <AudioGraphCanvas
          ariaLabel="Routing graph"
          model={model()}
          onModelChange={onModelChange}
          onViewportChange={onViewportChange}
        />
      </CanvasTestProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'viewport moving' }));
    expect(onViewportChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'finish viewport pan' }));
    expect(onViewportChange).toHaveBeenNthCalledWith(1, { x: 120, y: -30, zoom: 1.25 });
    fireEvent.click(screen.getByRole('button', { name: 'finish viewport zoom' }));
    expect(onViewportChange).toHaveBeenNthCalledWith(2, { x: 120, y: -30, zoom: 1.75 });
    expect(onViewportChange).toHaveBeenCalledTimes(2);
    expect(onModelChange).not.toHaveBeenCalled();
  });

  it('does not restore or emit again when a completed user gesture is acknowledged', () => {
    const onViewportChange = vi.fn();
    const view = render(canvas(undefined, onViewportChange));
    fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
    fireEvent.click(screen.getByRole('button', { name: 'finish viewport pan' }));
    const saved = { x: 120, y: -30, zoom: 1.25 };
    view.rerender(canvas(saved, onViewportChange));
    expect(viewportHarness.setViewport).not.toHaveBeenCalled();
    expect(onViewportChange).toHaveBeenCalledExactlyOnceWith(saved);
  });

  it('allows optional viewport callbacks to be omitted', () => {
    render(
      <CanvasTestProvider>
        <AudioGraphCanvas ariaLabel="Routing graph" model={model()} onModelChange={vi.fn()} />
      </CanvasTestProvider>,
    );
    expect(() =>
      fireEvent.click(screen.getByRole('button', { name: 'finish viewport pan' })),
    ).not.toThrow();
  });

  it('ignores a delayed move-end callback from an unmounted graph', () => {
    const onViewportChange = vi.fn();
    const view = render(canvas(undefined, onViewportChange));
    const delayedEnd = viewportHarness.lastMoveEnd;
    view.unmount();
    delayedEnd?.(new MouseEvent('mouseup'), { x: 500, y: 200, zoom: 1 });
    expect(onViewportChange).not.toHaveBeenCalled();
  });
});

describe('AudioGraphCanvas viewport persistence', () => {
  beforeEach(resetViewportHarness);

  it('saves a programmatic move such as Fit graph so a later mount restores the visible view', () => {
    const onViewportChange = vi.fn();
    render(canvas({ x: 140, y: 324, zoom: 0.05 }, onViewportChange));
    fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
    fireEvent.click(screen.getByRole('button', { name: 'finish programmatic viewport' }));
    expect(onViewportChange).toHaveBeenCalledExactlyOnceWith({ x: 120, y: -30, zoom: 1.75 });
  });
});

describe('AudioGraphCanvas persisted zoom limits', () => {
  beforeEach(resetViewportHarness);

  it('accepts both persisted zoom boundaries instead of clamping to React Flow defaults', () => {
    const onViewportChange = vi.fn();
    const minimum = { x: -500, y: 240, zoom: 0.05 };
    const maximum = { x: 900, y: -120, zoom: 8 };
    const view = render(canvas(minimum, onViewportChange));
    expect(screen.getByTestId('react-flow')).toHaveAttribute('data-min-zoom', '0.05');
    expect(screen.getByTestId('react-flow')).toHaveAttribute('data-max-zoom', '8');
    fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
    expect(viewportHarness.setViewport).toHaveBeenLastCalledWith(minimum, { duration: 0 });
    view.rerender(canvas(maximum, onViewportChange));
    expect(viewportHarness.setViewport).toHaveBeenLastCalledWith(maximum, { duration: 0 });
    expect(onViewportChange).not.toHaveBeenCalled();
  });
});

it('guides an empty graph to the add-node command without adding a second one', () => {
  const view = render(
    <CanvasTestProvider>
      <AudioGraphCanvas
        ariaLabel="Routing graph"
        model={{ ...model(), nodes: [], edges: [] }}
        nodeMenuItems={[{ id: 'test', label: 'Test source' }]}
        onModelChange={vi.fn()}
      />
    </CanvasTestProvider>,
  );
  expect(screen.getByText('Add apps, devices, effects and more.')).toBeInTheDocument();
  expect(screen.getAllByRole('button', { name: 'Add node' })).toHaveLength(1);
  view.rerender(
    <CanvasTestProvider>
      <AudioGraphCanvas
        ariaLabel="Routing graph"
        model={model()}
        nodeMenuItems={[{ id: 'test', label: 'Test source' }]}
        onModelChange={vi.fn()}
      />
    </CanvasTestProvider>,
  );
  expect(screen.queryByText('Add apps, devices, effects and more.')).not.toBeInTheDocument();
});

describe('AudioGraphCanvas discovery menu lifecycle', () => {
  it.each(['Add node', '', '   ', undefined])(
    'reports discovery menu lifecycle with label %s',
    async (label) => {
      const onOpened = vi.fn();
      render(
        <CanvasTestProvider>
          <AudioGraphCanvas
            addNodeLabel={label}
            ariaLabel="Routing graph"
            model={model()}
            nodeMenuItems={[{ id: 'test', label: 'Test source' }]}
            onModelChange={vi.fn()}
            onNodeMenuOpenedChange={onOpened}
          />
        </CanvasTestProvider>,
      );
      expect(onOpened).toHaveBeenLastCalledWith(false);
      const add = screen.getByRole('button', { name: 'Add node' });
      expect(add).toHaveTextContent('+');
      expect(add).not.toHaveTextContent('Add node');
      fireEvent.click(add);
      await waitFor(() => expect(onOpened).toHaveBeenLastCalledWith(true));
      fireEvent.click(await screen.findByRole('menuitem', { name: 'Test source' }));
      await waitFor(() => expect(onOpened).toHaveBeenLastCalledWith(false));
      fireEvent.contextMenu(screen.getByRole('group', { name: 'Processor' }));
      await waitFor(() => expect(onOpened).toHaveBeenLastCalledWith(true));
      fireEvent.pointerDown(screen.getByRole('group', { name: 'Source' }), {
        button: 0,
      });
      await waitFor(() => expect(onOpened).toHaveBeenLastCalledWith(false));
    },
  );
});

describe('AudioGraphCanvas add button placement', () => {
  function renderAddMenu(nodes: AudioGraphNode[] = []) {
    const onAddNode = vi.fn();
    const graph = model();
    graph.nodes.push(...nodes);
    render(
      <AudioGraphCanvas
        ariaLabel="Graph"
        model={graph}
        nodeMenuItems={[{ id: 'source:new', label: 'New source' }]}
        onAddNode={onAddNode}
        onModelChange={vi.fn()}
      />,
      { wrapper: CanvasTestProvider },
    );
    fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
    return { add: screen.getByRole('button', { name: 'Add node' }), onAddNode };
  }

  it('anchors the node when the menu opens on press start without a click', async () => {
    const { add, onAddNode } = renderAddMenu();
    const user = userEvent.setup();
    await user.pointer({ target: add, keys: '[MouseLeft>]' });
    const item = await screen.findByRole('menuitem', { name: 'New source' });
    expect(onAddNode).not.toHaveBeenCalled();
    await user.pointer({ target: add, keys: '[/MouseLeft]' });
    await user.click(item);
    expect(onAddNode).toHaveBeenCalledExactlyOnceWith(
      'source:new',
      { x: 132, y: 92 },
      expect.any(Function),
    );
  });

  it('passes Core placement to the insertion owner for a repeated add', async () => {
    const placed: AudioGraphNode = {
      id: 'placed',
      position: { x: 132, y: 92 },
      inputPorts: [],
      outputPorts: [],
      appearance: { category: 'Source', detail: 'App', icon: 'S', title: 'Placed' },
    };
    const { add, onAddNode } = renderAddMenu([placed]);
    act(() => add.focus());
    fireEvent.keyDown(add, { key: 'Enter' });
    fireEvent.keyUp(add, { key: 'Enter' });
    fireEvent.click(await screen.findByRole('menuitem', { name: 'New source' }));
    expect(onAddNode).toHaveBeenCalledExactlyOnceWith(
      'source:new',
      { x: 132, y: 92 },
      expect.any(Function),
    );
  });
});

describe('AudioGraphCanvas context menu wiring', () => {
  it('executes a real menu click as node deletion, including incident cables', async () => {
    const onModelChange = vi.fn();
    render(
      <CanvasTestProvider>
        <AudioGraphCanvas ariaLabel="Routing graph" model={model()} onModelChange={onModelChange} />
      </CanvasTestProvider>,
    );
    fireEvent.contextMenu(screen.getByRole('group', { name: 'Processor' }), {
      clientX: 140,
      clientY: 100,
    });
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Delete node' }));
    expect(onModelChange).toHaveBeenCalledWith(
      expect.objectContaining({ edges: [], revision: 2 }),
      'runtime',
    );
    expect(onModelChange.mock.calls[0]?.[0].nodes.map((node: { id: string }) => node.id)).toEqual([
      'source',
      'sink',
    ]);
  });

  it('executes the input-port menu as channel disconnection, not node removal', async () => {
    const onModelChange = vi.fn();
    render(
      <CanvasTestProvider>
        <AudioGraphCanvas ariaLabel="Routing graph" model={model()} onModelChange={onModelChange} />
      </CanvasTestProvider>,
    );
    fireEvent.contextMenu(screen.getByLabelText('Processor input, CH 1'), {
      clientX: 140,
      clientY: 100,
    });
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Disconnect channel' }));
    expect(onModelChange).toHaveBeenCalledWith(
      expect.objectContaining({ nodes: model().nodes, edges: [], revision: 2 }),
      'runtime',
    );
  });

  it('closes the context menu on another node pointer-down without changing the graph', async () => {
    const onModelChange = vi.fn();
    render(
      <CanvasTestProvider>
        <AudioGraphCanvas ariaLabel="Routing graph" model={model()} onModelChange={onModelChange} />
      </CanvasTestProvider>,
    );
    fireEvent.contextMenu(screen.getByRole('group', { name: 'Processor' }));
    await screen.findByRole('menu');
    fireEvent.pointerDown(screen.getByRole('group', { name: 'Source' }), {
      button: 0,
    });
    await waitFor(() => expect(screen.queryByRole('menu')).not.toBeInTheDocument());
    expect(onModelChange).not.toHaveBeenCalled();
  });
});

describe('AudioGraphCanvas', () => {
  it('adapts React Flow interactions into predictable graph commands', () => {
    const onModelChange = vi.fn();
    render(
      <CanvasTestProvider>
        <AudioGraphCanvas ariaLabel="Routing graph" model={model()} onModelChange={onModelChange} />
      </CanvasTestProvider>,
    );

    expect(screen.getByRole('region', { name: 'Routing graph' })).toBeInTheDocument();
    expect(screen.getByTestId('react-flow')).toHaveAttribute('data-connection-valid', 'true');
    expect(screen.getByTestId('react-flow')).toHaveAttribute('data-reconnect-valid', 'true');
    expect(screen.getByLabelText('Source output, CH 1')).toHaveAttribute(
      'data-handle-type',
      'source',
    );
    expect(screen.getByLabelText('Processor input, CH 1')).toHaveAttribute(
      'data-handle-type',
      'target',
    );
    expect(screen.getByTestId('background')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('group', { name: 'Source' }));
    fireEvent.click(screen.getByRole('group', { name: 'Processor' }));

    fireEvent.click(screen.getByRole('button', { name: 'invalid connect' }));
    fireEvent.click(screen.getByRole('button', { name: 'invalid reconnect' }));
    fireEvent.click(screen.getByRole('button', { name: 'finish valid reconnect' }));
    expect(onModelChange).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'drop reconnect in empty space' }));
    expect(onModelChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ edges: [], revision: 2 }),
      'runtime',
    );

    fireEvent.click(screen.getByRole('button', { name: 'connect' }));
    expect(onModelChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ revision: 2, edges: expect.arrayContaining([expect.any(Object)]) }),
      'runtime',
    );
    fireEvent.click(screen.getByRole('button', { name: 'reconnect' }));
    fireEvent.click(screen.getByRole('button', { name: 'delete selected' }));
    fireEvent.click(screen.getByRole('button', { name: 'delete node' }));
    fireEvent.click(screen.getByRole('button', { name: 'delete double click' }));
    fireEvent.click(screen.getByRole('button', { name: 'move' }));
    expect(onModelChange).toHaveBeenCalledTimes(7);
    expect(onModelChange).toHaveBeenLastCalledWith(
      expect.objectContaining({
        nodes: expect.arrayContaining([
          expect.objectContaining({ id: 'processor', position: { x: 220, y: 140 } }),
        ]),
      }),
      'layout',
    );
  });
});

function selectionCanvas() {
  const graph = model();
  const source = graph.nodes[0];
  const processor = graph.nodes[1];
  assert(source && processor);
  source.outputPorts = [5, 8, 9, 10].map((channelIndex) => ({
    id: `channel-${channelIndex}`,
    channelIndex,
    label: 'Source output',
  }));
  processor.outputPorts = [0, 1].map((channelIndex) => ({
    id: `other-${channelIndex}`,
    channelIndex,
    label: 'Other output',
  }));
  const changed = vi.fn();
  const view = render(
    <AudioGraphCanvas ariaLabel="Graph" model={graph} onModelChange={changed} />,
    { wrapper: CanvasTestProvider },
  );
  for (const handle of view.container.querySelectorAll<HTMLElement>('[data-audio-port-side]')) {
    const isSource = handle.dataset.audioNodeId === 'source';
    const ports = isSource ? source.outputPorts : processor.outputPorts;
    const index = ports.findIndex((port) => port.id === handle.dataset.audioPortId);
    vi.spyOn(handle, 'getBoundingClientRect').mockReturnValue(
      new DOMRect(isSource ? 10 : 100, 10 + index * 20, 2, 2),
    );
  }
  return { ...view, changed };
}

it('shows live channel ranges and counts before committing a one-node output selection', () => {
  const view = selectionCanvas();
  portPointer('pointerdown', { x: 0, y: 0 });
  portPointer('pointermove', { x: 20, y: 55 });
  expect(screen.getByRole('status')).toHaveTextContent('CH 6, 9–10 · 3 ch');
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(3);
  expect(view.changed).not.toHaveBeenCalled();
  portPointer('pointerup', { x: 20, y: 55 });
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(3);
});

it('locks a drag to its first source even when the rectangle later covers other nodes', () => {
  const view = selectionCanvas();
  portPointer('pointerdown', { x: 0, y: 0 });
  portPointer('pointermove', { x: 20, y: 55 });
  portPointer('pointermove', { x: 120, y: 55 });
  portPointer('pointerup', { x: 120, y: 55 });
  const selected = view.container.querySelectorAll<HTMLElement>('[data-port-selected]');
  expect(selected).toHaveLength(3);
  expect(Array.from(selected).every((port) => port.dataset.audioNodeId === 'source')).toBe(true);
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  selectPortRectangle({ x: 90, y: 0 }, { x: 120, y: 55 }, true);
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(3);
  selectPortRectangle({ x: 90, y: 0 }, { x: 120, y: 55 });
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(2);
  expect(
    Array.from(view.container.querySelectorAll<HTMLElement>('[data-port-selected]')).every(
      (port) => port.dataset.audioNodeId === 'processor',
    ),
  ).toBe(true);
});

it('restores the committed selection on pointer cancellation and Escape during a drag', () => {
  const view = selectionCanvas();
  selectPortRectangle({ x: 0, y: 0 }, { x: 20, y: 35 });
  for (const cancel of ['pointercancel', 'Escape']) {
    portPointer('pointerdown', { x: 0, y: 45 }, true);
    portPointer('pointermove', { x: 20, y: 80 }, true);
    expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(4);
    if (cancel === 'Escape')
      fireEvent.keyDown(screen.getByRole('region', { name: 'Graph' }), { key: 'Escape' });
    else portPointer('pointercancel', { x: 20, y: 80 });
    expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(2);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    portPointer('pointerup', { x: 20, y: 80 });
    expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(2);
  }
  fireEvent.keyDown(screen.getByRole('region', { name: 'Graph' }), { key: 'Escape' });
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(0);
});

it('hides feedback for empty drags and ignores releases from unrelated pointers', () => {
  selectionCanvas();
  portPointer('pointerdown', { x: 0, y: 0 });
  portPointer('pointermove', { x: 20, y: 35 });
  expect(screen.getByRole('status')).toHaveTextContent('CH 6, 9 · 2 ch');
  portPointer('pointerup', { x: 20, y: 35 }, false, 2);
  expect(screen.getByRole('status')).toHaveTextContent('CH 6, 9 · 2 ch');
  portPointer('pointerup', { x: 20, y: 35 });
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  portPointer('pointerdown', { x: 0, y: 300 });
  portPointer('pointermove', { x: 20, y: 320 });
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  portPointer('pointerup', { x: 20, y: 320 });
});

it('exposes one named node focus target and preserves independent node controls', () => {
  const view = render(
    <AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={vi.fn()} />,
    {
      wrapper: CanvasTestProvider,
    },
  );
  expect(screen.getByRole('region', { name: 'Graph' })).toHaveAttribute('tabindex', '-1');
  for (const title of ['Source', 'Processor', 'Sink']) {
    const node = screen.getByRole('group', { name: title });
    expect(node).toHaveAttribute('tabindex', '0');
    expect(node.querySelector('[data-node-face]')).not.toHaveAttribute('tabindex');
    expect(node.querySelector('[data-node-face]')?.tagName).toBe('DIV');
  }
  expect(view.container.querySelectorAll('[data-node-face]')).toHaveLength(3);
});

it('adds a node from the canvas context menu and fits the initialized viewport without changing audio', async () => {
  const onAddNode = vi.fn();
  const onModelChange = vi.fn();
  render(
    <AudioGraphCanvas
      ariaLabel="Graph"
      model={model()}
      nodeMenuItems={[{ id: 'source:new', label: 'New source' }]}
      onAddNode={onAddNode}
      onModelChange={onModelChange}
    />,
    { wrapper: CanvasTestProvider },
  );
  fireEvent.click(screen.getByRole('button', { name: 'initialize viewport' }));
  fireEvent.contextMenu(screen.getByRole('button', { name: 'Graph background' }), {
    clientX: 120,
    clientY: 80,
  });
  fireEvent.click(await screen.findByRole('menuitem', { name: 'New source' }));
  expect(onAddNode).toHaveBeenCalledExactlyOnceWith(
    'source:new',
    { x: 120, y: 80 },
    expect.any(Function),
  );
  fireEvent.contextMenu(screen.getByRole('button', { name: 'Graph background' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'Fit graph' }));
  expect(viewportHarness.fitView).toHaveBeenCalledWith({ padding: 0.15, maxZoom: 1 });
  expect(onModelChange).not.toHaveBeenCalled();
});

it('preserves the node keyboard focus while Escape clears its selection', async () => {
  const onModelChange = vi.fn();
  render(<AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={onModelChange} />, {
    wrapper: CanvasTestProvider,
  });
  fireEvent.contextMenu(screen.getByRole('button', { name: 'Graph background' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'Select all' }));
  expect(screen.getByTestId('react-flow')).toHaveAttribute(
    'data-selected-nodes',
    'source,processor,sink',
  );
  const node = screen.getByRole('group', { name: 'Processor' });
  node.focus();
  fireEvent.keyDown(node, { key: 'Escape' });
  expect(document.activeElement).toBe(node);
  expect(screen.getByTestId('react-flow')).toHaveAttribute('data-selected-nodes', '');
  expect(onModelChange).not.toHaveBeenCalled();
});

it('keeps native text context menus from triggering canvas actions', () => {
  const onModelChange = vi.fn();
  render(
    <AudioGraphCanvas
      ariaLabel="Graph"
      model={model()}
      onModelChange={onModelChange}
      renderNodeContent={(node) =>
        node.id === 'source' ? <input aria-label="Native editable label" /> : null
      }
    />,
    { wrapper: CanvasTestProvider },
  );
  fireEvent.contextMenu(screen.getByRole('textbox', { name: 'Native editable label' }));
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  expect(onModelChange).not.toHaveBeenCalled();
});

it('prepares the exact node context for a touch long press and ignores empty touches', async () => {
  vi.useFakeTimers();
  const onModelChange = vi.fn();
  const view = render(
    <AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={onModelChange} />,
    { wrapper: CanvasTestProvider },
  );
  try {
    const node = screen.getByRole('group', { name: 'Processor' });
    fireEvent.touchStart(node, { touches: [] });
    await act(() => vi.advanceTimersByTimeAsync(600));
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    fireEvent.touchStart(node, { touches: [{ clientX: 140, clientY: 100 }] });
    await act(() => vi.advanceTimersByTimeAsync(600));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete node' }));
    expect(onModelChange).toHaveBeenCalledWith(expect.objectContaining({ edges: [] }), 'runtime');
  } finally {
    view.unmount();
    vi.useRealTimers();
  }
});

it.each([false, true])(
  'renders a free-drag connection preview with target available=%s without inventing target channels',
  (targetAvailable) => {
    previewHarness.enabled = true;
    previewHarness.targetAvailable = targetAvailable;
    previewHarness.targetPortId = '';
    render(<AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={vi.fn()} />, {
      wrapper: CanvasTestProvider,
    });
    expect(screen.getByLabelText('Connection preview').querySelectorAll('path')).toHaveLength(1);
  },
);

it('forwards node gain and plugin parameter edits to the corresponding native owner callback', () => {
  const graph = model();
  const source = graph.nodes[0];
  const processor = graph.nodes[1];
  assert(source && processor);
  source.appearance.gainControl = { label: 'Source gain', valueDb: 0 };
  processor.appearance.parameterControls = [
    { id: 'ratio', label: 'Ratio', minimum: 1, maximum: 10, step: 1, suffix: ':1', value: 3 },
  ];
  const gain = vi.fn();
  const parameter = vi.fn();
  render(
    <AudioGraphCanvas
      ariaLabel="Graph"
      model={graph}
      onModelChange={vi.fn()}
      onNodeGainChange={gain}
      onNodeParameterChange={parameter}
    />,
    { wrapper: CanvasTestProvider },
  );
  const sourceView = screen.getByRole('group', { name: 'Source' });
  fireEvent.click(
    within(sourceView).getByRole('button', { name: (name) => name.endsWith('Options') }),
  );
  const gainSlider = screen.getByRole('slider', { name: 'Source gain' });
  gainSlider.focus();
  fireEvent.keyDown(gainSlider, { key: 'ArrowLeft' });
  expect(gain).toHaveBeenCalledExactlyOnceWith('source', -0.5);
  const ratio = screen.getByRole('slider', { name: 'Ratio' });
  ratio.focus();
  fireEvent.keyDown(ratio, { key: 'ArrowRight' });
  expect(parameter).toHaveBeenCalledExactlyOnceWith('processor', 'ratio', 4);
});

it('sends a device-name edit from that device node to its native owner', () => {
  const graph = model();
  const sink = graph.nodes[2];
  assert(sink);
  sink.appearance.endpointControl = {
    displayName: 'Sink',
    nameLabel: 'Device name',
    renderChannels: 2,
    captureChannels: 2,
  };
  const changed = vi.fn();
  render(
    <AudioGraphCanvas
      ariaLabel="Graph"
      model={graph}
      onModelChange={vi.fn()}
      onNodeEndpointChange={changed}
    />,
    { wrapper: CanvasTestProvider },
  );
  fireEvent.click(
    within(screen.getByRole('group', { name: 'Sink' })).getByRole('button', {
      name: (name) => name.endsWith('Options'),
    }),
  );
  const name = screen.getByRole('textbox', { name: 'Device name' });
  fireEvent.change(name, { target: { value: 'Monitoring' } });
  fireEvent.blur(name);
  expect(changed).toHaveBeenCalledExactlyOnceWith('sink', {
    displayName: 'Monitoring',
    renderChannels: 2,
    captureChannels: 2,
  });
});

it('opens the selected native editor, reports its failure, retries it and follows its native size', () => {
  const graph = model();
  const processor = graph.nodes[1];
  assert(processor);
  processor.appearance.hasProcessorEditor = true;
  const retry = vi.fn();
  const props = {
    ariaLabel: 'Graph',
    model: graph,
    onModelChange: vi.fn(),
    onProcessorEditorRetry: retry,
    processorEditorLabels: {
      open: 'Open editor',
      retry: 'Retry editor',
      failure: {
        no_editor: 'No editor',
        display_unavailable: 'No display',
        host_window_failed: 'Window failed',
        plugin_rejected: 'Plug-in rejected',
        worker_unavailable: 'Host stopped',
      },
    },
  };
  const view = render(<AudioGraphCanvas {...props} />, { wrapper: CanvasTestProvider });
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Open editor') }));
  expect(retry).toHaveBeenCalledExactlyOnceWith('processor');
  view.rerender(
    <AudioGraphCanvas
      {...props}
      processorEditorStatuses={
        new Map([
          [
            'processor',
            {
              protocolVersion: 1,
              nodeId: 'processor',
              state: 'failed',
              failure: 'plugin_rejected',
              resizable: false,
              width: null,
              height: null,
            },
          ],
        ])
      }
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent('Plug-in rejected');
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Retry editor') }));
  expect(retry).toHaveBeenCalledTimes(2);
  view.rerender(
    <AudioGraphCanvas
      {...props}
      processorEditorStatuses={
        new Map([
          [
            'processor',
            {
              protocolVersion: 1,
              nodeId: 'processor',
              state: 'open',
              failure: null,
              resizable: true,
              width: 720,
              height: 420,
            },
          ],
        ])
      }
    />,
  );
  expect(
    screen.queryByRole('button', { name: (name) => name.endsWith('Retry editor') }),
  ).not.toBeInTheDocument();
  const shell = screen.getByRole('group', { name: 'Processor' }).firstElementChild;
  expect(shell).toHaveStyle({ width: '720px' });
});

it('selects a cable through Shift box selection without changing routing or losing an existing selection', async () => {
  const original = globalThis.DOMPoint;
  globalThis.DOMPoint = class {
    constructor(
      public x = 0,
      public y = 0,
    ) {}
    matrixTransform() {
      return this;
    }
  } as unknown as typeof DOMPoint;
  try {
    const changed = vi.fn();
    const view = render(
      <AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={changed} />,
      {
        wrapper: CanvasTestProvider,
      },
    );
    const group = document.createElementNS('http://www.w3.org/2000/svg', 'g');
    group.classList.add('react-flow__edge');
    group.dataset.id = 'source-to-processor';
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.classList.add('react-flow__edge-path');
    Object.defineProperties(path, {
      getScreenCTM: { value: () => ({}) },
      getTotalLength: { value: () => 10 },
      getPointAtLength: { value: (length: number) => ({ x: 12 + length / 10, y: 12 }) },
      getBoundingClientRect: { value: () => new DOMRect(10, 10, 10, 10) },
    });
    group.append(path);
    view.container.querySelector('[data-testid="react-flow"]')?.append(group);
    const pane = screen.getByRole('button', { name: 'Graph background' });
    for (const kind of ['pointerdown', 'pointerup', 'pointerdown', 'pointerup']) {
      fireEvent(
        pane,
        new PointerEvent(kind, {
          bubbles: true,
          clientX: kind === 'pointerdown' ? 10 : 20,
          clientY: kind === 'pointerdown' ? 10 : 20,
          shiftKey: true,
        }),
      );
      if (kind === 'pointerup') await act(() => new Promise(requestAnimationFrame));
    }
    expect(screen.getByTestId('react-flow')).toHaveAttribute(
      'data-selected-edges',
      'source-to-processor',
    );
    expect(changed).not.toHaveBeenCalled();
  } finally {
    globalThis.DOMPoint = original;
  }
});

it('keeps a single free-drag preview while the originating node disappears from the current graph', () => {
  const graph = model();
  graph.nodes = graph.nodes.filter((node) => node.id !== 'processor');
  graph.edges = [];
  previewHarness.enabled = true;
  previewHarness.targetAvailable = false;
  const changed = vi.fn();
  render(<AudioGraphCanvas ariaLabel="Graph" model={graph} onModelChange={changed} />, {
    wrapper: CanvasTestProvider,
  });
  expect(screen.getByLabelText('Connection preview').querySelectorAll('path')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'connect' }));
  expect(changed).not.toHaveBeenCalled();
});

it('ignores React Flow initialization that finishes after its canvas has unmounted', () => {
  resetViewportHarness();
  const view = render(canvas({ x: 40, y: 20, zoom: 1.5 }, vi.fn()));
  const initialize = viewportHarness.initialize;
  assert(initialize);
  view.unmount();
  initialize();
  expect(viewportHarness.setViewport).not.toHaveBeenCalled();
});

it('keeps live measurement distinct from an explicit device-node resize without a disclosure panel', () => {
  const changed = vi.fn();
  const view = render(
    <AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={changed} />,
    {
      wrapper: CanvasTestProvider,
    },
  );
  fireEvent.click(screen.getByRole('button', { name: 'measure source' }));
  expect(view.container.querySelector('[data-id="source"]')).toHaveAttribute(
    'data-flow-measured-height',
    '250',
  );
  expect(changed).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'resize sink' }));
  expect(changed).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({
      nodes: expect.arrayContaining([
        expect.objectContaining({ id: 'sink', size: { width: 320, height: 250 } }),
      ]),
    }),
    'layout',
  );
});

it('keeps the context menu open for right-button gestures and closes it for ordinary selection', async () => {
  render(<AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={vi.fn()} />, {
    wrapper: CanvasTestProvider,
  });
  fireEvent.contextMenu(screen.getByRole('group', { name: 'Processor' }));
  const menu = await screen.findByRole('menu');
  fireEvent.pointerDown(screen.getByRole('button', { name: 'Graph background' }), { button: 2 });
  expect(menu).toBeInTheDocument();
  fireEvent.pointerDown(screen.getByRole('button', { name: 'Graph background' }), { button: 0 });
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
});

it('retains one preview for a React Flow connection with no named source handle', () => {
  previewHarness.enabled = true;
  previewHarness.sourcePortId = null;
  previewHarness.targetAvailable = false;
  render(<AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={vi.fn()} />, {
    wrapper: CanvasTestProvider,
  });
  expect(screen.getByLabelText('Connection preview').querySelectorAll('path')).toHaveLength(1);
});

it.each(['missing port', 'missing device'] as const)(
  'refuses a selected-channel batch when the target has a %s during a native drag',
  (targetMode) => {
    const graph = model();
    const processor = graph.nodes[1];
    assert(processor);
    processor.outputPorts.push({ id: 'out-2', label: 'Second output' });
    const changed = vi.fn();
    const view = render(
      <AudioGraphCanvas ariaLabel="Graph" model={graph} onModelChange={changed} />,
      {
        wrapper: CanvasTestProvider,
      },
    );
    view.container
      .querySelectorAll<HTMLElement>('[data-audio-port-side]')
      .forEach((handle, index) => {
        const selected =
          handle.dataset.audioNodeId === 'processor' && handle.dataset.audioPortSide === 'output';
        vi.spyOn(handle, 'getBoundingClientRect').mockReturnValue(
          new DOMRect(selected ? 10 : 100, index * 4 + 1, 2, 2),
        );
      });
    selectPortRectangle({ x: 0, y: 0 }, { x: 20, y: 70 });
    expect(view.container.querySelectorAll('[data-port-selected]')).toHaveLength(2);
    previewHarness.enabled = true;
    if (targetMode === 'missing port') previewHarness.targetPortId = 'removed';
    const latest = {
      ...graph,
      nodes:
        targetMode === 'missing device'
          ? graph.nodes.filter((node) => node.id !== 'sink')
          : graph.nodes,
    };
    view.rerender(<AudioGraphCanvas ariaLabel="Graph" model={latest} onModelChange={changed} />);
    expect(screen.getByLabelText('Connection preview').querySelectorAll('path')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'connect' }));
    expect(changed).not.toHaveBeenCalled();
  },
);

it('keeps EQ resize minimum aligned with its shared 320px display width', () => {
  const graph = model();
  const processor = graph.nodes.find((node) => node.id === 'processor');
  if (!processor) throw new Error('Processor fixture is missing');
  processor.appearance.equalizer = { bands: [] };
  processor.size = { width: 228, height: 160 };
  const view = render(
    <AudioGraphCanvas ariaLabel="Graph" model={graph} onModelChange={vi.fn()} />,
    { wrapper: CanvasTestProvider },
  );
  const eq = view.container.querySelector('[data-equalizer]');
  expect(eq).toHaveStyle({ width: '320px' });
  expect(eq?.querySelector('[data-resize-grip]')).toHaveAttribute('data-min-width', '320');
  const normal = view.container.querySelector('[data-id="source"] [data-resize-grip]');
  expect(normal).toHaveAttribute('data-min-width', '180');
});

it('forwards an EQ graph drag atomically with its node identity while arrows retain single-value editing', () => {
  const graph = model();
  const processor = graph.nodes.find((node) => node.id === 'processor');
  assert(processor);
  processor.appearance.equalizer = {
    sampleRate: 48000,
    bands: [
      { id: 'mid', label: 'Mid', frequencyId: 'midFrequencyHz', gainId: 'midDb', qId: 'midQ' },
    ],
  };
  processor.appearance.parameterControls = [
    {
      id: 'midFrequencyHz',
      label: 'Frequency',
      value: 1000,
      minimum: 20,
      maximum: 20000,
      step: 1,
      suffix: 'Hz',
      scale: 'logarithmic',
    },
    { id: 'midDb', label: 'Gain', value: 0, minimum: -24, maximum: 24, step: 0.1, suffix: 'dB' },
    {
      id: 'midQ',
      label: 'Q',
      value: Number(Math.SQRT1_2.toFixed(3)),
      minimum: 0.1,
      maximum: 10,
      step: 0.001,
      suffix: '',
    },
  ];
  const batch = vi.fn();
  const single = vi.fn();
  const changed = vi.fn();
  render(
    <AudioGraphCanvas
      ariaLabel="Graph"
      model={graph}
      onModelChange={changed}
      onNodeParameterChange={single}
      onNodeParametersChange={batch}
    />,
    { wrapper: CanvasTestProvider },
  );
  const handle = screen.getByRole('button', { name: /^1 Mid, 1000 Hz/ });
  const plot = handle.parentElement;
  assert(plot);
  plot.setPointerCapture = vi.fn();
  plot.hasPointerCapture = vi.fn(() => true);
  plot.releasePointerCapture = vi.fn();
  vi.spyOn(plot, 'getBoundingClientRect').mockReturnValue(new DOMRect(10, 20, 300, 120));
  fireEvent.pointerDown(handle, { button: 0, pointerId: 93 });
  fireEvent.pointerMove(plot, {
    pointerId: 93,
    clientX: 10 + frequencyPosition(2200),
    clientY: 20 + gainPosition(6),
  });
  fireEvent.pointerUp(plot, { pointerId: 93 });
  expect(batch).toHaveBeenCalledExactlyOnceWith('processor', [
    { id: 'midFrequencyHz', value: 2200 },
    { id: 'midDb', value: 6 },
  ]);
  expect(single).not.toHaveBeenCalled();
  expect(changed).not.toHaveBeenCalled();
  expect(plot.releasePointerCapture).toHaveBeenCalledWith(93);
  fireEvent.keyDown(handle, { key: 'ArrowRight' });
  expect(single).toHaveBeenCalledExactlyOnceWith('processor', 'midFrequencyHz', 1001);
  expect(batch).toHaveBeenCalledOnce();
});

it('offers display-name editing on title double-click and F2 without an audio/model edit', () => {
  const graph = model();
  const first = graph.nodes[0];
  assert(first);
  const request = vi.fn();
  const change = vi.fn();
  const view = render(
    <AudioGraphCanvas
      ariaLabel="Graph"
      model={graph}
      onModelChange={change}
      onNodeRenameRequest={request}
    />,
    { wrapper: CanvasTestProvider },
  );
  const node = screen.getByRole('group', { name: first.appearance.title });
  const title = node.querySelector('[data-node-title]');
  assert(title);
  fireEvent.doubleClick(title);
  expect(request).toHaveBeenLastCalledWith(first.id);
  expect(node).toHaveFocus();
  fireEvent.keyDown(node, { key: 'F2' });
  expect(request).toHaveBeenCalledTimes(2);
  fireEvent.keyDown(title, { key: 'F2' });
  const face = node.querySelector('[data-node-face]');
  assert(face);
  fireEvent.doubleClick(face);
  const text = title.firstChild;
  assert(text);
  fireEvent.doubleClick(text);
  expect(request).toHaveBeenCalledTimes(3);
  fireEvent.click(screen.getByRole('button', { name: 'non-element node double-click' }));
  expect(request).toHaveBeenCalledTimes(3);
  node.removeAttribute('data-id');
  fireEvent.keyDown(node, { key: 'F2' });
  expect(request).toHaveBeenCalledTimes(3);
  node.classList.remove('react-flow__node');
  fireEvent.doubleClick(title);
  expect(request).toHaveBeenCalledTimes(4);
  expect(change).not.toHaveBeenCalled();
  view.unmount();
  render(<AudioGraphCanvas ariaLabel="Graph" model={graph} onModelChange={change} />, {
    wrapper: CanvasTestProvider,
  });
  fireEvent.keyDown(screen.getByRole('group', { name: first.appearance.title }), {
    key: 'F2',
  });
  expect(request).toHaveBeenCalledTimes(4);
});

it('names cables, ports and React Flow keyboard help in the current language', async () => {
  await i18n.changeLanguage('en');
  const view = render(
    <AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={vi.fn()} />,
    {
      wrapper: CanvasTestProvider,
    },
  );
  expect(screen.getByTestId('edge-label')).toHaveTextContent('Source CH 1 → Processor CH 1');
  expect(screen.getByRole('img', { name: 'Source output, CH 1' })).not.toHaveAttribute(
    'tabindex',
    '0',
  );
  const twoBuses = model();
  const [sourceNode, processor] = twoBuses.nodes;
  assert(sourceNode && processor);
  sourceNode.outputPorts = [
    { id: 'out', label: 'Out A', channelIndex: 0 },
    { id: 'alt', label: 'Out B', channelIndex: 0 },
  ];
  processor.inputPorts = [
    { id: 'in', label: 'Main', channelIndex: 0 },
    { id: 'side', label: 'Sidechain', channelIndex: 0 },
  ];
  view.rerender(<AudioGraphCanvas ariaLabel="Graph" model={twoBuses} onModelChange={vi.fn()} />);
  expect(screen.getByTestId('edge-label')).toHaveTextContent(
    'Source · Out A CH 1 → Processor · Main CH 1',
  );
  processor.inputPorts = [
    { id: 'in', label: 'Main', channelIndex: 0 },
    { id: 'side', label: 'Main', channelIndex: 0 },
  ];
  view.rerender(
    <AudioGraphCanvas ariaLabel="Graph" model={{ ...twoBuses }} onModelChange={vi.fn()} />,
  );
  expect(screen.getByTestId('edge-label')).toHaveTextContent(
    'Source · Out A CH 1 → Processor · Main (1) CH 1',
  );
  expect(screen.getByRole('img', { name: 'Main (2), CH 1' })).toBeInTheDocument();
  expect(screen.getByTestId('node-description')).toHaveTextContent(/arrow keys.*Shift/);
  expect(screen.getByTestId('node-moved')).toHaveTextContent(
    'Moved the selected node left. New position: x 12, y 34.',
  );

  await act(() => i18n.changeLanguage('ko-KR'));
  view.rerender(<AudioGraphCanvas ariaLabel="Graph" model={model()} onModelChange={vi.fn()} />);
  expect(screen.getByTestId('edge-description')).toHaveTextContent('케이블을 선택합니다');
  expect(screen.getByTestId('node-moved')).toHaveTextContent('왼쪽으로 옮겼습니다');
  await act(() => i18n.changeLanguage('en'));
});

describe('AudioGraphCanvas keyboard channel connection', () => {
  function renderCanvas() {
    const onModelChange = vi.fn();
    render(
      <AudioGraphCanvas ariaLabel="Routing graph" model={model()} onModelChange={onModelChange} />,
      { wrapper: CanvasTestProvider },
    );
    return onModelChange;
  }

  it('opens the node menu with Shift+F10 and connects in one runtime commit', async () => {
    const onModelChange = renderCanvas();
    const node = screen.getByRole('group', { name: 'Processor' });
    act(() => node.focus());
    await expect(deleteHarness.allowed()).resolves.toBe(true);
    const key = fireEvent.keyDown(node, { key: 'F10', shiftKey: true });
    expect(key).toBe(false);
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Connect channels…' }));
    const dialog = await screen.findByRole('dialog', { name: 'Connect channels' });
    expect(dialog).toHaveTextContent('From Processor');
    // Deletion keys must not reach the selected node while the dialog owns focus.
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    await expect(deleteHarness.allowed()).resolves.toBe(false);
    fireEvent.click(within(dialog).getByRole('button', { name: /Destination$/ }));
    fireEvent.click(screen.getByRole('option', { name: 'Sink' }));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Connect' }));
    expect(onModelChange).toHaveBeenCalledOnce();
    expect(onModelChange).toHaveBeenCalledWith(
      expect.objectContaining({
        edges: expect.arrayContaining([
          expect.objectContaining({
            sourceNodeId: 'processor',
            sourcePortId: 'out',
            targetNodeId: 'sink',
            targetPortId: 'in',
          }),
        ]),
      }),
      'runtime',
    );
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await waitFor(() => expect(node).toHaveFocus());
  });

  it('opens the canvas menu from the ContextMenu key and ignores repeats and nested controls', async () => {
    renderCanvas();
    const canvas = screen.getByRole('region', { name: 'Routing graph' });
    fireEvent.keyDown(canvas, { key: 'ContextMenu', repeat: true });
    fireEvent.keyDown(canvas, { key: 'F10' });
    fireEvent.keyDown(canvas, { key: 'F10', shiftKey: true, ctrlKey: true });
    fireEvent.keyDown(canvas, { key: 'ContextMenu', isComposing: true });
    fireEvent.keyDown(screen.getByLabelText('Processor input, CH 1'), { key: 'ContextMenu' });
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    fireEvent.keyDown(canvas, { key: 'ContextMenu' });
    expect(await screen.findByRole('menuitem', { name: 'Fit graph' })).toBeInTheDocument();
    expect(screen.getByRole('menu', { name: 'Actions' })).toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Connect channels…' })).toBeNull();
  });

  it("offers the connection only for one node or one node's output channels", async () => {
    renderCanvas();
    fireEvent.contextMenu(screen.getByRole('group', { name: 'Sink' }));
    await screen.findByRole('menuitem', { name: 'Delete node' });
    expect(screen.queryByRole('menuitem', { name: 'Connect channels…' })).toBeNull();
    fireEvent.contextMenu(screen.getByLabelText('Processor input, CH 1'));
    await screen.findByRole('menuitem', { name: 'Disconnect channel' });
    expect(screen.queryByRole('menuitem', { name: 'Connect channels…' })).toBeNull();
    fireEvent.contextMenu(screen.getByLabelText('Source output, CH 1'));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Connect channels…' }));
    expect(await screen.findByRole('dialog', { name: 'Connect channels' })).toHaveTextContent(
      'Selected channels: CH 1',
    );
  });

  it('lets graph deletion proceed only while focus is inside the graph', async () => {
    renderCanvas();
    await expect(deleteHarness.allowed()).resolves.toBe(false);
    act(() => screen.getByRole('group', { name: 'Source' }).focus());
    await expect(deleteHarness.allowed()).resolves.toBe(true);
    act(() => (document.activeElement as HTMLElement).blur());
    await expect(deleteHarness.allowed()).resolves.toBe(false);
  });
});

it('reports a node channel layout choice with the node identity', () => {
  const graph = model();
  const processor = graph.nodes[1];
  assert(processor);
  processor.appearance.channelLayoutControl = {
    label: 'Channel layout',
    value: 'stereo',
    options: [
      { value: 'mono', label: 'Mono' },
      { value: 'stereo', label: 'Stereo' },
    ],
  };
  const onNodeChannelLayoutChange = vi.fn();
  render(
    <AudioGraphCanvas
      ariaLabel="Graph"
      model={graph}
      onModelChange={vi.fn()}
      onNodeChannelLayoutChange={onNodeChannelLayoutChange}
    />,
    { wrapper: CanvasTestProvider },
  );
  fireEvent.click(screen.getByRole('button', { name: 'Processor: Options' }));
  fireEvent.click(screen.getByRole('button', { name: /Channel layout$/ }));
  fireEvent.click(screen.getByRole('option', { name: 'Mono' }));
  expect(onNodeChannelLayoutChange).toHaveBeenCalledWith('processor', 'mono');
});
