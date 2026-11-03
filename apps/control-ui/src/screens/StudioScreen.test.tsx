import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../core/domain/control/graph';
import type { AudioSessionStatus } from '../core/domain/control/status';
import { MidiContext, MidiPanelContext } from '../core/midi/MidiContext';
import type { ControlPort } from '../core/ports/control/ControlPort';
import '../test-support/integration';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import {
  unknownNativeFormats,
  virtualPolicy,
  virtualSpec,
} from '../features/routing-workspace/virtualEndpointFixtures';
import { StudioScreen } from './StudioScreen';

const callbacks = vi.hoisted(
  () => new Map<ControlPort, (document: RuntimeGraphDocument) => void>(),
);
interface WorkspaceBinding {
  controlPort: ControlPort;
  graphId?: string;
  sessionDocument?: RuntimeGraphDocument;
  audioSession?: AudioSessionStatus;
  onBusyChange: (busy: boolean) => void;
  onTitleChange: (title: string) => void;
  deviceDriverAction: {
    change: (
      endpointId: string,
      update: import('../core/ui/AudioNodeEndpointOptions').EndpointUpdate,
    ) => void;
    enabled: boolean;
    error: { endpointId: string | null; message: string } | null;
  };
  headerActions: React.ReactNode;
}
const workspaceBindings = vi.hoisted(() => new Map<ControlPort, WorkspaceBinding>());
const workspaceRenders = vi.hoisted(() => [] as WorkspaceBinding[]);
const sessionDisabled = vi.hoisted(() => new Map<ControlPort, boolean>());
vi.mock('../features/audio-session/AudioPauseControl', () => ({
  AudioPauseControl: ({
    controlPort,
    onApplied,
    disabled,
  }: {
    controlPort: ControlPort;
    onApplied: (document: RuntimeGraphDocument) => void;
    disabled: boolean;
  }) => {
    callbacks.set(controlPort, onApplied);
    sessionDisabled.set(controlPort, disabled);
    return null;
  },
}));
// Captures the View → Show/Hide Graph List command, which jsdom has no native menu for.
const graphListCommand = vi.hoisted(() => ({ toggle: () => {} }));
vi.mock('../core/shell/useNativeGraphListCommand', () => ({
  useNativeGraphListCommand: (_visible: boolean, onToggle: () => void) => {
    graphListCommand.toggle = onToggle;
  },
}));
vi.mock('../features/routing-workspace/RoutingWorkspace', () => ({
  RoutingWorkspace: (binding: WorkspaceBinding) => {
    workspaceBindings.set(binding.controlPort, binding);
    workspaceRenders.push(binding);
    return (
      <>
        <output data-testid="document">{JSON.stringify(binding.sessionDocument)}</output>
        {binding.headerActions}
      </>
    );
  },
}));
function port(revision: number) {
  return new StorybookMockControlPort({
    graph: { revision, nodes: [], edges: [], schemaVersion: 2 },
    graphStatus: { protocolVersion: 1, activeRevision: revision, submittedRevision: revision },
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
function view(controlPort: ControlPort, driverSession?: DriverSession) {
  return (
    <MixRelayProvider>
      <I18nProvider>
        <StudioScreen controlPort={controlPort} driverSession={driverSession} />
      </I18nProvider>
    </MixRelayProvider>
  );
}

function newGraphShortcut(
  target: EventTarget,
  modifiers: { ctrlKey?: boolean; metaKey?: boolean } = {},
) {
  const event = new KeyboardEvent('keydown', {
    bubbles: true,
    cancelable: true,
    key: 'N',
    shiftKey: true,
    ...modifiers,
  });
  act(() => target.dispatchEvent(event));
  return event;
}

function devicePort() {
  const controlPort = Object.assign(port(1), { setVirtualEndpointSampleRate: vi.fn() });
  let sampleRate = 48000;
  let document: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
  const nativeStatus: AudioSessionStatus = {
    format: { sampleRate: 96000, blockFrames: 32 },
    clockAuthority: 'device',
    observedDeviceSampleRate: 96000,
    issue: null,
    transportLatencyFrames: null,
    capabilities: {
      backend: 'core_audio',
      supportedSampleRates: [44100, 48000, 96000],
      supportedBlockFrames: [32, 128],
      canReconfigureLive: true,
      hardwareInput: true,
      hardwareOutput: true,
      virtualEndpoint: true,
      applicationCapture: true,
      sampleRateConversion: true,
      driftCorrection: true,
    },
    virtualEndpointSettings: {
      ...virtualPolicy,
      devices: [
        {
          nativeDeviceIds: [],
          nativeFormats: unknownNativeFormats,
          spec: { ...virtualSpec, id: 'device-a' },
          canSetSampleRate: true,
          observedSampleRate: 48000,
          supportedSampleRates: [44100, 48000],
        },
      ],
    },
  };
  const endpointSettings = nativeStatus.virtualEndpointSettings;
  if (!endpointSettings) throw new Error('Missing device fixture');
  vi.spyOn(controlPort, 'getGraph').mockImplementation(async () => document);
  vi.spyOn(controlPort, 'getGraphStatus').mockImplementation(async () => ({
    protocolVersion: 1,
    activeRevision: document.revision,
    submittedRevision: document.revision,
  }));
  vi.spyOn(controlPort, 'getStatus').mockImplementation(async () => ({
    protocolVersion: 1,
    driverState: 'ready',
    sampleRate: 96000,
    bufferFrames: 32,
    underruns: 0,
    overruns: 0,
    audioSession: {
      ...nativeStatus,
      virtualEndpointSettings: {
        ...endpointSettings,
        devices: endpointSettings.devices.map((device) => ({
          ...device,
          observedSampleRate: sampleRate,
        })),
      },
    },
  }));
  controlPort.setVirtualEndpointSampleRate.mockImplementation(async ({ sampleRate: rate }) => {
    sampleRate = rate;
    document = { ...document, revision: 2 };
    return { protocolVersion: 1, revision: 2, state: 'active' };
  });
  return controlPort;
}

it('saves and opens a workspace and allows another graph', async () => {
  const file = vi.fn().mockResolvedValue(null);
  const controlPort = Object.assign(port(1), { workspaceFile: file });
  const bounds = vi
    .spyOn(HTMLElement.prototype, 'getBoundingClientRect')
    .mockReturnValue(new DOMRect(0, 0, 200, 120));
  const width = vi.spyOn(document.documentElement, 'clientWidth', 'get').mockReturnValue(800);
  const height = vi.spyOn(document.documentElement, 'clientHeight', 'get').mockReturnValue(600);
  render(view(controlPort));
  await waitFor(() => expect(workspaceBindings.get(controlPort)?.graphId).toBe('graph-1'));
  fireEvent.click(screen.getByRole('button', { name: 'File' }));
  const save = screen.getByRole('menuitem', { name: 'Save' });
  expect(save).not.toHaveAttribute('aria-disabled', 'true');
  fireEvent.click(save);
  await waitFor(() =>
    expect(file).toHaveBeenCalledWith(expect.objectContaining({ action: 'save' })),
  );
  await waitFor(() => expect(screen.queryByRole('menu')).not.toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'File' }));
  const open = screen.getByRole('menuitem', { name: 'Open…' });
  expect(open).not.toHaveAttribute('aria-disabled', 'true');
  fireEvent.click(open);
  await waitFor(() =>
    expect(file).toHaveBeenCalledWith(expect.objectContaining({ action: 'open' })),
  );
  fireEvent.click(screen.getByRole('button', { name: 'New graph' }));
  expect(screen.getByRole('dialog', { name: 'New graph' })).toBeInTheDocument();
  bounds.mockRestore();
  width.mockRestore();
  height.mockRestore();
});

it('keeps the new-graph launcher stable across background saves but guards creation', async () => {
  const controlPort = port(1);
  const mutate = vi.spyOn(controlPort, 'mutateGraphCatalog');
  render(view(controlPort));
  const launcher = await screen.findByRole('button', { name: 'New graph' });
  await waitFor(() => expect(launcher).toBeEnabled());
  expect(launcher).toHaveAttribute('aria-keyshortcuts', 'Control+Shift+N');
  // The command carries a visible name, so it needs no tooltip to explain itself.
  expect(launcher).toHaveTextContent('New graph');
  const binding = workspaceBindings.get(controlPort);
  if (!binding) throw new Error('Workspace not mounted');
  let transitions = 0;
  let wasDisabled = launcher.hasAttribute('disabled');
  for (let cycle = 0; cycle < 20; cycle += 1) {
    for (const busy of [true, false]) {
      act(() => binding.onBusyChange(busy));
      const disabled = launcher.hasAttribute('disabled');
      if (disabled !== wasDisabled) transitions += 1;
      wasDisabled = disabled;
    }
  }
  expect(transitions).toBe(0);
  act(() => binding.onBusyChange(true));
  fireEvent.click(launcher);
  expect(screen.getByRole('dialog', { name: 'New graph' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  expect(mutate).not.toHaveBeenCalled();
  act(() => binding.onBusyChange(false));
  expect(screen.getByRole('button', { name: 'Create' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it.each([
  ['MacIntel', { metaKey: true }],
  ['Linux x86_64', { ctrlKey: true }],
] as const)(
  'preserves text editing and uses the platform new-graph modifier on %s',
  async (platform, modifier) => {
    const identity = vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
    render(view(port(1)));
    const launcher = await screen.findByRole('button', { name: 'New graph' });
    expect(launcher).toHaveAttribute(
      'aria-keyshortcuts',
      `${platform === 'MacIntel' ? 'Meta' : 'Control'}+Shift+N`,
    );

    const input = document.createElement('input');
    input.type = 'text';
    const textarea = document.createElement('textarea');
    const editable = document.createElement('div');
    editable.setAttribute('contenteditable', 'true');
    const nested = document.createElement('span');
    editable.append(nested);
    document.body.append(input, textarea, editable);
    try {
      for (const target of [input, textarea, nested]) {
        const event = newGraphShortcut(target, modifier);
        expect(event.defaultPrevented, `shortcut intercepted from ${target.tagName}`).toBe(false);
        expect(screen.queryByRole('dialog', { name: 'New graph' })).not.toBeInTheDocument();
      }

      const claimed = new KeyboardEvent('keydown', {
        bubbles: true,
        cancelable: true,
        ...modifier,
        key: 'n',
        shiftKey: true,
      });
      claimed.preventDefault();
      act(() => document.body.dispatchEvent(claimed));
      expect(screen.queryByRole('dialog', { name: 'New graph' })).not.toBeInTheDocument();

      const wrongModifier = platform === 'MacIntel' ? { ctrlKey: true } : { metaKey: true };
      expect(newGraphShortcut(document.body, wrongModifier).defaultPrevented).toBe(false);
      expect(screen.queryByRole('dialog', { name: 'New graph' })).not.toBeInTheDocument();
      const canvasEvent = newGraphShortcut(document.body, modifier);
      expect(canvasEvent.defaultPrevented).toBe(true);
      expect(screen.getByRole('dialog', { name: 'New graph' })).toBeInTheDocument();
    } finally {
      input.remove();
      textarea.remove();
      editable.remove();
      identity.mockRestore();
    }
  },
);

it('keeps the graph list fixed on wide windows without an on-screen toggle', async () => {
  const controlPort = port(1);
  render(view(controlPort));
  await screen.findByRole('button', { name: 'Untitled Graph' });
  expect(screen.getByRole('navigation', { name: 'Graphs' })).toBeVisible();
  // Carbon: the header menu button exists only while the list collapses.
  expect(screen.queryByRole('button', { name: 'Graphs' })).not.toBeInTheDocument();
});

it('sends device edits through the independent Driver owner during graph title edits', async () => {
  const controlPort = devicePort();
  const binding = () => {
    const current = workspaceBindings.get(controlPort);
    if (!current) throw new Error('Workspace not mounted');
    return current;
  };
  const changeDevice = vi.fn().mockResolvedValue(null);
  const driverSession: DriverSession = {
    port: null,
    snapshot: {
      protocolVersion: 1,
      revision: 4,
      state: 'ready',
      installedVersion: '0.1.0',
      settings: virtualPolicy,
    },
    busy: false,
    error: null,
    endpointId: null,
    refresh: vi.fn().mockResolvedValue(undefined),
    mutate: vi.fn().mockResolvedValue(null),
    changeDevice,
  };
  render(view(controlPort, driverSession));
  await waitFor(() => expect(binding().deviceDriverAction.enabled).toBe(true));
  act(() => binding().onTitleChange('Unsaved title'));
  expect(binding().deviceDriverAction.enabled).toBe(true);
  expect(sessionDisabled.get(controlPort)).toBe(true);
  const update = {
    displayName: 'Monitor',
    renderChannels: 2,
    captureChannels: 2,
    sampleFormat: 'signed24' as const,
  };
  await act(async () => {
    binding().deviceDriverAction.change('device-a', update);
  });
  expect(changeDevice).toHaveBeenCalledExactlyOnceWith('device-a', update);
  expect(controlPort.setVirtualEndpointSampleRate).not.toHaveBeenCalled();
});

describe('Studio session document ownership', () => {
  it('never shows the previous graph snapshot when selection keeps the same audio revision', async () => {
    const controlPort = port(7);
    const first = { schemaVersion: 2 as const, revision: 7, nodes: [], edges: [] };
    const second = {
      ...first,
      nodes: [{ id: 'other-device', type: 'audio_device_output', label: null, config: {} }],
    };
    const catalog = {
      protocolVersion: 1,
      catalogRevision: 1,
      activeId: 'a',
      graphs: [
        { id: 'a', name: 'Graph A' },
        { id: 'b', name: 'Graph B' },
      ],
      document: first,
    };
    vi.spyOn(controlPort, 'getGraphCatalog').mockResolvedValue(catalog);
    vi.spyOn(controlPort, 'mutateGraphCatalog').mockResolvedValue({
      ...catalog,
      catalogRevision: 2,
      activeId: 'b',
      document: second,
    });
    const rendered = render(view(controlPort));
    await screen.findByRole('button', { name: 'Graph B' });
    act(() => callbacks.get(controlPort)?.({ ...first, revision: 8 }));
    const oldCompletion = callbacks.get(controlPort);
    workspaceRenders.length = 0;
    fireEvent.click(screen.getByRole('button', { name: 'Graph B' }));
    await waitFor(() => expect(workspaceBindings.get(controlPort)?.graphId).toBe('b'));
    const nextView = workspaceRenders.filter((binding) => binding.graphId === 'b');
    expect(nextView.length).toBeGreaterThan(0);
    expect(nextView.every((binding) => binding.sessionDocument === second)).toBe(true);
    act(() => oldCompletion?.({ ...first, revision: 9 }));
    expect(workspaceBindings.get(controlPort)?.sessionDocument).toBe(second);
    rendered.unmount();
  });
  it('does not adopt a higher revision document from the previously connected engine', async () => {
    const a = port(100),
      b = port(1);
    const rendered = render(view(a));
    await waitFor(() => expect(screen.getByTestId('document')).toHaveTextContent('"revision":100'));
    const lateCallback = callbacks.get(a);
    rendered.rerender(view(b));
    await waitFor(() => expect(screen.getByTestId('document')).toHaveTextContent('"revision":1,'));
    act(() => callbacks.get(b)?.({ revision: 2, nodes: [], edges: [], schemaVersion: 2 }));
    expect(screen.getByTestId('document')).toHaveTextContent('"revision":2,');
    act(() => lateCallback?.({ revision: 101, nodes: [], edges: [], schemaVersion: 2 }));
    expect(screen.getByTestId('document')).toHaveTextContent('"revision":2,');
    expect(screen.getByTestId('document')).not.toHaveTextContent('"revision":101');
    rendered.unmount();
  });
});

it('adopts a confirmed native session revision while the catalog is still loading', async () => {
  const controlPort = port(1);
  const catalog = await controlPort.getGraphCatalog();
  const pending = Promise.withResolvers<typeof catalog>();
  vi.spyOn(controlPort, 'getGraphCatalog').mockReturnValue(pending.promise);
  render(view(controlPort));
  const apply = callbacks.get(controlPort);
  if (!apply) throw new Error('Expected public audio session callback');
  const document = { ...catalog.document, revision: 2 };
  act(() => apply(document));
  expect(workspaceBindings.get(controlPort)?.sessionDocument).toEqual(document);
  await act(async () => pending.resolve({ ...catalog, document }));
});

it('starts the overlay sidebar closed on narrow screens and closes it after a choice', async () => {
  const listeners = new Set<() => void>();
  const media = { matches: true };
  const matchMedia = vi.spyOn(window, 'matchMedia').mockImplementation(
    (query: string) =>
      ({
        get matches() {
          return media.matches;
        },
        media: query,
        addEventListener: (_: string, listener: () => void) => listeners.add(listener),
        removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
      }) as unknown as MediaQueryList,
  );
  const controlPort = port(2);
  const document = { schemaVersion: 2 as const, revision: 2, nodes: [], edges: [] };
  const catalog = {
    protocolVersion: 1,
    catalogRevision: 1,
    activeId: 'a',
    graphs: [
      { id: 'a', name: 'Graph A' },
      { id: 'b', name: 'Graph B' },
    ],
    document,
  };
  vi.spyOn(controlPort, 'getGraphCatalog').mockResolvedValue(catalog);
  vi.spyOn(controlPort, 'mutateGraphCatalog').mockResolvedValue({
    ...catalog,
    catalogRevision: 2,
    activeId: 'b',
  });
  try {
    render(view(controlPort));
    const toggle = await screen.findByRole('button', { name: 'Graphs' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('navigation', { name: 'Graphs' })).not.toBeInTheDocument();

    fireEvent.click(toggle);
    const list = screen.getByRole('navigation', { name: 'Graphs' });
    fireEvent.click(within(list).getByRole('button', { name: 'Graph B' }));
    await waitFor(() =>
      expect(screen.queryByRole('navigation', { name: 'Graphs' })).not.toBeInTheDocument(),
    );

    media.matches = false;
    act(() => {
      for (const listener of listeners) listener();
    });
    // Wide again: the fixed list returns and the menu button goes away.
    expect(screen.queryByRole('button', { name: 'Graphs' })).not.toBeInTheDocument();
    const graphA = within(screen.getByRole('navigation', { name: 'Graphs' })).getByRole('button', {
      name: 'Graph A',
    });
    act(() => graphA.focus());
    expect(graphA).toHaveFocus();
    media.matches = true;
    act(() => {
      for (const listener of listeners) listener();
    });
    expect(screen.queryByRole('navigation', { name: 'Graphs' })).not.toBeInTheDocument();
    // Focus that lived in the closed list moves to the header menu button.
    const menu = screen.getByRole('button', { name: 'Graphs' });
    expect(menu).toHaveFocus();
    expect(menu.closest('header')).toContainElement(
      screen.getByRole('heading', { name: 'MixRelay' }),
    );
  } finally {
    matchMedia.mockRestore();
  }
});

function breakpoint(initial: boolean) {
  const listeners = new Set<() => void>();
  const media = { matches: initial };
  const matchMedia = vi.mocked(window.matchMedia);
  const original = matchMedia.getMockImplementation();
  matchMedia.mockImplementation(
    (query: string) =>
      ({
        get matches() {
          return media.matches;
        },
        media: query,
        addEventListener: (_: string, listener: () => void) => listeners.add(listener),
        removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
      }) as unknown as MediaQueryList,
  );
  return {
    set(narrow: boolean) {
      media.matches = narrow;
      act(() => {
        for (const listener of listeners) listener();
      });
    },
    restore() {
      if (original) matchMedia.mockImplementation(original);
    },
  };
}

it('moves focus from the menu button to the fixed list when the window widens', async () => {
  const width = breakpoint(true);
  try {
    render(view(port(1)));
    const menu = await screen.findByRole('button', { name: 'Graphs' });
    fireEvent.click(menu);
    act(() => menu.focus());
    width.set(false);
    expect(screen.queryByRole('button', { name: 'Graphs' })).not.toBeInTheDocument();
    const list = screen.getByRole('navigation', { name: 'Graphs' });
    expect(list).toContainElement(document.activeElement as HTMLElement);
  } finally {
    width.restore();
  }
});

it('does not pull focus back after the user deliberately left the menu button', async () => {
  const width = breakpoint(true);
  try {
    render(view(port(1)));
    const menu = await screen.findByRole('button', { name: 'Graphs' });
    act(() => menu.focus());
    // A real blur and a later resize are separate tasks; let the blur settle first.
    await act(async () => menu.blur());
    width.set(false);
    expect(document.activeElement).toBe(document.body);
  } finally {
    width.restore();
  }
});

function enabledListControl() {
  const control = within(screen.getByRole('navigation', { name: 'Graphs' }))
    .getAllByRole('button')
    .find((button) => !button.hasAttribute('disabled'));
  if (!control) throw new Error('Expected a graph list control');
  return control;
}

it('leaves focus in the list when widening keeps the list on screen', async () => {
  const width = breakpoint(true);
  try {
    render(view(port(1)));
    fireEvent.click(await screen.findByRole('button', { name: 'Graphs' }));
    const item = enabledListControl();
    act(() => item.focus());
    width.set(false);
    expect(item).toHaveFocus();
  } finally {
    width.restore();
  }
});

it('lets focus that left the shell stay where it went when the list hides', async () => {
  render(view(port(1)));
  await screen.findByRole('navigation', { name: 'Graphs' });
  const item = enabledListControl();
  act(() => item.focus());
  // Moving within the shell keeps tracking; leaving it ends the shell's claim.
  act(() => screen.getByRole('region', { name: 'Routing workspace' }).focus());
  act(() => item.focus());
  const outside = document.createElement('button');
  document.body.append(outside);
  try {
    act(() => outside.focus());
    act(() => graphListCommand.toggle());
    expect(outside).toHaveFocus();
  } finally {
    outside.remove();
  }
});

it('does not move focus when a blur and an open land in the same task', async () => {
  const width = breakpoint(true);
  try {
    render(view(port(1)));
    const menu = await screen.findByRole('button', { name: 'Graphs' });
    act(() => menu.focus());
    act(() => {
      menu.blur();
      graphListCommand.toggle();
    });
    expect(screen.getByRole('navigation', { name: 'Graphs' })).toBeVisible();
    expect(document.activeElement).toBe(document.body);
  } finally {
    width.restore();
  }
});

it('keeps focus when the View command hides the list, then widening hides the overlay', async () => {
  const width = breakpoint(false);
  try {
    render(view(port(1)));
    await screen.findByRole('navigation', { name: 'Graphs' });
    act(() => graphListCommand.toggle());
    expect(screen.queryByRole('navigation', { name: 'Graphs' })).not.toBeInTheDocument();
    width.set(true);
    fireEvent.click(screen.getByRole('button', { name: 'Graphs' }));
    const item = within(screen.getByRole('navigation', { name: 'Graphs' }))
      .getAllByRole('button')
      .find((button) => !button.hasAttribute('disabled'));
    if (!item) throw new Error('Expected a graph list control');
    act(() => item.focus());
    expect(item).toHaveFocus();
    width.set(false);
    // The list stays hidden by the View command; focus lands on the workspace.
    expect(screen.queryByRole('navigation', { name: 'Graphs' })).not.toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Routing workspace' })).toHaveFocus();
  } finally {
    width.restore();
  }
});

it('skips an inert workspace and keeps focus in the shell when the list hides', async () => {
  render(view(port(1)));
  const list = await screen.findByRole('navigation', { name: 'Graphs' });
  const item = within(list)
    .getAllByRole('button')
    .find((button) => !button.hasAttribute('disabled'));
  if (!item) throw new Error('Expected a graph list control');
  act(() => item.focus());
  expect(item).toHaveFocus();
  screen.getByRole('region', { name: 'Routing workspace' }).setAttribute('inert', '');
  act(() => graphListCommand.toggle());
  const focused = document.activeElement as HTMLElement;
  expect(focused.closest('header')).not.toBeNull();
  expect(focused.tagName).toBe('BUTTON');
});

it('keeps the overlay sidebar open when a graph switch fails', async () => {
  const matchMedia = vi.spyOn(window, 'matchMedia').mockImplementation(
    (query: string) =>
      ({
        matches: true,
        media: query,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      }) as unknown as MediaQueryList,
  );
  const controlPort = port(2);
  vi.spyOn(controlPort, 'getGraphCatalog').mockResolvedValue({
    protocolVersion: 1,
    catalogRevision: 1,
    activeId: 'a',
    graphs: [
      { id: 'a', name: 'Graph A' },
      { id: 'b', name: 'Graph B' },
    ],
    document: { schemaVersion: 2, revision: 2, nodes: [], edges: [] },
  });
  vi.spyOn(controlPort, 'mutateGraphCatalog').mockRejectedValue(new Error('graph_invalid'));
  try {
    render(view(controlPort));
    fireEvent.click(await screen.findByRole('button', { name: 'Graphs' }));
    const list = screen.getByRole('navigation', { name: 'Graphs' });
    fireEvent.click(within(list).getByRole('button', { name: 'Graph B' }));
    await waitFor(() => expect(controlPort.mutateGraphCatalog).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.getByRole('navigation', { name: 'Graphs' })).toBeVisible();
    // The failed choice is over: a later refresh that activates B does not close the sidebar.
    vi.mocked(controlPort.getGraphCatalog).mockResolvedValue({
      protocolVersion: 1,
      catalogRevision: 3,
      activeId: 'b',
      graphs: [
        { id: 'a', name: 'Graph A' },
        { id: 'b', name: 'Graph B' },
      ],
      document: { schemaVersion: 2, revision: 3, nodes: [], edges: [] },
    });
    const loads = vi.mocked(controlPort.getGraphCatalog).mock.calls.length;
    fireEvent.click(within(list).getByRole('button', { name: 'Reload graphs' }));
    await waitFor(() =>
      expect(vi.mocked(controlPort.getGraphCatalog).mock.calls.length).toBeGreaterThan(loads),
    );
    await waitFor(() =>
      expect(within(list).getByRole('button', { name: 'Graph B' })).toHaveAttribute(
        'aria-current',
        'page',
      ),
    );
    expect(screen.getByRole('navigation', { name: 'Graphs' })).toBeVisible();
  } finally {
    matchMedia.mockRestore();
  }
});

it.each([false, true])(
  'map mode ignores native graph-list commands and preserves prior visibility (%s)',
  async (visible) => {
    const matchMedia = vi.spyOn(window, 'matchMedia').mockImplementation(
      (query) =>
        ({
          matches: false,
          media: query,
          addEventListener: vi.fn(),
          removeEventListener: vi.fn(),
        }) as unknown as MediaQueryList,
    );
    const controlPort = port(1);
    const frame = (active: boolean) => (
      <MidiContext.Provider
        value={{
          active,
          status: null,
          selected: null,
          toggle: vi.fn(),
          assign: vi.fn(),
          remove: vi.fn(),
        }}
      >
        <MidiPanelContext.Provider value={<aside aria-label="Mappings">Mappings</aside>}>
          {view(controlPort)}
        </MidiPanelContext.Provider>
      </MidiContext.Provider>
    );
    const rendered = render(frame(false));
    try {
      await screen.findByRole('navigation', { name: 'Graphs' });
      if (!visible) act(() => graphListCommand.toggle());
      rendered.rerender(frame(true));
      expect(screen.queryByRole('navigation', { name: 'Graphs' })).not.toBeInTheDocument();
      act(() => graphListCommand.toggle());
      expect(screen.getByRole('complementary', { name: 'Mappings' })).toBeVisible();
      rendered.rerender(frame(false));
      expect(Boolean(screen.queryByRole('navigation', { name: 'Graphs' }))).toBe(visible);
    } finally {
      rendered.unmount();
      matchMedia.mockRestore();
    }
  },
);
