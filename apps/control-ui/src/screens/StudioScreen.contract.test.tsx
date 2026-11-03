import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import { ControlPortError } from '../core/domain/control/errors';
import type { VirtualEndpointSpec } from '../core/domain/control/status';
import type { ControlPort } from '../core/ports/control/ControlPort';
import '../test-support/integration';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { I18nProvider, i18n } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import type { GraphCreateDialog } from '../features/graphs/GraphCreateDialog';
import type { GraphList } from '../features/graphs/GraphList';
import type { RoutingWorkspaceProps } from '../features/routing-workspace/RoutingWorkspace';
import { virtualPolicy, virtualSpec } from '../features/routing-workspace/virtualEndpointFixtures';
import { StudioScreen } from './StudioScreen';

const harness = vi.hoisted(() => ({
  workspace: null as RoutingWorkspaceProps | null,
  creation: null as ComponentProps<typeof GraphCreateDialog> | null,
  list: null as ComponentProps<typeof GraphList> | null,
}));
vi.mock('../features/graphs/GraphCreateDialog', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../features/graphs/GraphCreateDialog')>();
  return {
    GraphCreateDialog: (props: ComponentProps<typeof GraphCreateDialog>) => {
      harness.creation = props;
      return <actual.GraphCreateDialog {...props} />;
    },
  };
});
vi.mock('../features/graphs/GraphList', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../features/graphs/GraphList')>();
  return {
    GraphList: (props: ComponentProps<typeof GraphList>) => {
      harness.list = props;
      return <actual.GraphList {...props} />;
    },
  };
});
vi.mock('../features/routing-workspace/RoutingWorkspace', () => ({
  RoutingWorkspace: (props: RoutingWorkspaceProps) => {
    harness.workspace = props;
    return (
      <>
        {props.notice}
        {props.headerActions}
      </>
    );
  },
}));
vi.mock('../features/audio-session/AudioPauseControl', () => ({ AudioPauseControl: () => null }));
function native() {
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
function view(controlPort: ControlPort, driverSession?: DriverSession) {
  return (
    <MixRelayProvider>
      <I18nProvider>
        <StudioScreen controlPort={controlPort} driverSession={driverSession} />
      </I18nProvider>
    </MixRelayProvider>
  );
}
function driver(): DriverSession {
  return {
    port: null,
    snapshot: {
      protocolVersion: 1,
      revision: 1,
      state: 'ready',
      installedVersion: '0.1.0',
      settings: virtualPolicy,
    },
    busy: false,
    error: null,
    endpointId: null,
    refresh: vi.fn().mockResolvedValue(undefined),
    changeDevice: vi.fn(),
    mutate: vi.fn().mockResolvedValue(null),
  };
}
function workspace() {
  if (!harness.workspace) throw new Error('Expected workspace');
  return harness.workspace;
}
beforeEach(async () => {
  harness.workspace = null;
  harness.creation = null;
  harness.list = null;
  await i18n.changeLanguage('en');
});
it('opens and closes the collapsed graph list with one header menu button', async () => {
  // Swap the shared setup mock's implementation and put it back afterwards.
  const matchMedia = vi.mocked(window.matchMedia);
  const original = matchMedia.getMockImplementation();
  matchMedia.mockImplementation(
    (query: string) =>
      ({
        matches: true,
        media: query,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      }) as unknown as MediaQueryList,
  );
  try {
    render(view(native()));
    const toggle = await screen.findByRole('button', { name: 'Graphs' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('navigation', { name: 'Graphs' })).not.toBeInTheDocument();
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByRole('navigation', { name: 'Graphs' })).toBeVisible();
    fireEvent.click(toggle);
    expect(screen.queryByRole('navigation', { name: 'Graphs' })).not.toBeInTheDocument();
  } finally {
    if (original) matchMedia.mockImplementation(original);
  }
});
it('creates a named graph through confirmed native catalog activation and can cancel the next dialog', async () => {
  const port = native();
  const mutation = vi.spyOn(port, 'mutateGraphCatalog');
  render(view(port));
  await waitFor(() => expect(screen.getByRole('button', { name: 'New graph' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'New graph' }));
  fireEvent.change(await screen.findByRole('textbox', { name: 'Graph name' }), {
    target: { value: '  Session two  ' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  await waitFor(() => expect(mutation).toHaveBeenCalledTimes(1));
  expect(mutation.mock.calls[0]?.[0]).toMatchObject({ action: 'create', name: 'Session two' });
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'New graph' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(mutation).toHaveBeenCalledTimes(1);
});
it('reports catalog storage failure and lets the user recover by explicitly reloading graphs', async () => {
  const port = native();
  const read = vi.spyOn(port, 'getGraphCatalog').mockRejectedValueOnce(
    new ControlPortError(500, {
      code: 'control_unavailable',
      parameters: { resource: 'graph_catalog' },
    }),
  );
  render(view(port));
  fireEvent.click(await screen.findByRole('button', { name: 'Reload graphs' }));
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  await waitFor(() =>
    expect(screen.queryByRole('button', { name: 'Reload graphs' })).not.toBeInTheDocument(),
  );
});
it('shows an active graph persistence failure without discarding its native document', async () => {
  const port = native();
  const catalog = await port.getGraphCatalog();
  vi.spyOn(port, 'getGraphCatalog').mockResolvedValue({
    ...catalog,
    persistenceError: 'write_failed',
  });
  render(view(port));
  expect(
    await screen.findByText(
      'Audio is active, but the graph could not be saved. Restarting may lose these changes.',
    ),
  ).toBeVisible();
  expect(workspace().sessionDocument).toEqual(catalog.document);
});
it('uses the Driver creation readback and returns null when it has no matching confirmed device', async () => {
  const session = driver();
  const mutate = vi.mocked(session.mutate);
  const spec: VirtualEndpointSpec = { ...virtualSpec, id: 'new-device' };
  mutate.mockResolvedValueOnce({
    ...session.snapshot,
    protocolVersion: 1,
    revision: 2,
    state: 'ready',
    installedVersion: '0.1.0',
    settings: {
      ...virtualPolicy,
      devices: virtualPolicy.devices.map((device) => ({ ...device, spec })),
    },
  });
  render(view(native(), session));
  const action = workspace().deviceDriverAction;
  if (!action) throw new Error('Expected Driver action');
  let created: VirtualEndpointSpec | null = null;
  await act(async () => {
    created = await action.create(spec);
  });
  expect(created).toEqual(spec);
  expect(mutate).toHaveBeenCalledExactlyOnceWith({ action: 'create', spec });
  await act(async () => expect(await action.create(spec)).toBeNull());
});
it('uses an empty catalog for a first graph and reports a disabled Driver controller honestly', async () => {
  const port = native();
  const catalog = await port.getGraphCatalog();
  vi.spyOn(port, 'getGraphCatalog').mockResolvedValue({ ...catalog, graphs: [] });
  const session = {
    ...driver(),
    port: null,
    snapshot: null,
    busy: true,
    error: 'Driver unavailable',
  };
  render(view(port, session));
  await waitFor(() => expect(screen.getByRole('button', { name: 'New graph' })).toBeEnabled());
  expect(workspace().deviceDriverAction).toMatchObject({
    enabled: false,
    settings: undefined,
    error: { endpointId: null, message: 'Driver unavailable' },
  });
});

it('reports a generic catalog rejection with the standard user-facing recovery text', async () => {
  const port = native();
  vi.spyOn(port, 'getGraphCatalog').mockRejectedValue(
    new ControlPortError(403, { code: 'graph_invalid', parameters: {} }),
  );
  render(view(port));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    i18n.t('routing.errors.graph_invalid'),
  );
});
it('deletes the selected graph using confirmed native catalog activation', async () => {
  const port = native();
  const mutation = vi.spyOn(port, 'mutateGraphCatalog');
  render(view(port));
  fireEvent.click(
    await screen.findByRole('button', { name: (name) => name.endsWith('Delete graph') }),
  );
  const confirmation = await screen.findByRole('dialog', { name: 'Delete graph' });
  fireEvent.click(
    within(confirmation).getByRole('button', { name: (name) => name.endsWith('Delete graph') }),
  );
  await waitFor(() => expect(mutation).toHaveBeenCalledTimes(1));
  expect(mutation.mock.calls[0]?.[0]).toMatchObject({ action: 'delete', id: 'graph-1' });
});

it('guards catalog writes from public Feature callbacks while a workspace command owns the engine', async () => {
  const port = native();
  const mutation = vi.spyOn(port, 'mutateGraphCatalog');
  render(view(port));
  await waitFor(() => expect(screen.getByRole('button', { name: 'New graph' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'New graph' }));
  await screen.findByRole('dialog', { name: 'New graph' });
  act(() => workspace().onBusyChange?.(true));
  await waitFor(() => expect(harness.creation?.busy).toBe(true));
  const creation = harness.creation;
  const list = harness.list;
  if (!creation || !list) throw new Error('Expected public Feature adapters');
  act(() => {
    creation.onCreate('Late event');
    list.onDelete('graph-1');
  });
  expect(mutation).not.toHaveBeenCalled();
});
it('opens another graph from the global new-graph shortcut', async () => {
  const port = native();
  render(view(port));
  await waitFor(() => expect(workspace().graphId).toBe('graph-1'));
  fireEvent.keyDown(window, { key: 'n', ctrlKey: true, shiftKey: true });
  expect(screen.getByRole('dialog', { name: 'New graph' })).toBeInTheDocument();
});
