import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../core/domain/control/graph';
import type { MixRelayStatus } from '../core/domain/control/status';
import type { ControlPort } from '../core/ports/control/ControlPort';
import type { DriverPort } from '../core/ports/DriverPort';
import type { UpdatesPort } from '../core/ports/UpdatesPort';
import '../test-support/integration';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { I18nProvider, i18n, resolveLocale } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import { readyDriver } from '../features/driver-status/driverFixtures';
import { App } from './App';

function controlPortWithStatus(
  controlPort: ControlPort,
  getStatus: ControlPort['getStatus'],
): ControlPort {
  return {
    recordingSettings: controlPort.recordingSettings.bind(controlPort),
    setRecordingSettings: controlPort.setRecordingSettings.bind(controlPort),
    recorderStatus: controlPort.recorderStatus.bind(controlPort),
    controlRecorder: controlPort.controlRecorder.bind(controlPort),
    applyGraph: controlPort.applyGraph.bind(controlPort),
    getGraph: controlPort.getGraph.bind(controlPort),
    getGraphStatus: controlPort.getGraphStatus.bind(controlPort),
    getMeters: controlPort.getMeters.bind(controlPort),
    getProcessorEditor: controlPort.getProcessorEditor.bind(controlPort),
    getProcessorEditorFrame: controlPort.getProcessorEditorFrame.bind(controlPort),
    sendProcessorEditorInput: controlPort.sendProcessorEditorInput.bind(controlPort),
    getProcessors: controlPort.getProcessors.bind(controlPort),
    getStatus,
    rescanProcessors: controlPort.rescanProcessors.bind(controlPort),
    setProcessorEditor: controlPort.setProcessorEditor.bind(controlPort),
    setProcessorParameter: controlPort.setProcessorParameter.bind(controlPort),
    getNodeControls: controlPort.getNodeControls.bind(controlPort),
    updateNodeControl: controlPort.updateNodeControl.bind(controlPort),
    retryProcessor: controlPort.retryProcessor.bind(controlPort),
    getGraphCatalog: controlPort.getGraphCatalog.bind(controlPort),
    mutateGraphCatalog: controlPort.mutateGraphCatalog.bind(controlPort),
    getGraphLayout: controlPort.getGraphLayout.bind(controlPort),
    setGraphLayout: controlPort.setGraphLayout.bind(controlPort),
  };
}

const graph: RuntimeGraphDocument = { edges: [], nodes: [], revision: 1, schemaVersion: 2 };
const status: MixRelayStatus = {
  bufferFrames: null,
  driverState: 'not_installed',
  overruns: 0,
  protocolVersion: 1,
  sampleRate: null,
  underruns: 0,
};
let controlPort: StorybookMockControlPort;
beforeEach(() => {
  controlPort = new StorybookMockControlPort({
    graph,
    graphStatus: { activeRevision: 1, protocolVersion: 1, submittedRevision: 1 },
    status,
  });
});

describe('App composition', () => {
  it('renders the Core to Feature to Screen composition', async () => {
    await i18n.changeLanguage('en');
    render(
      <MixRelayProvider>
        <I18nProvider>
          <App controlPort={controlPort} />
        </I18nProvider>
      </MixRelayProvider>,
    );

    expect(screen.getByRole('heading', { name: 'MixRelay' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Graphs' })).not.toBeInTheDocument();
    expect(await screen.findByRole('button', { name: 'Untitled Graph' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    const newGraph = screen.getByRole('button', { name: 'New graph' });
    await waitFor(() => expect(newGraph).toBeEnabled());
    fireEvent.click(newGraph);
    const dialog = screen.getByRole('dialog', { name: 'New graph' });
    expect(dialog).toBeInTheDocument();
    fireEvent.change(within(dialog).getByRole('textbox', { name: 'Graph name' }), {
      target: { value: 'Broadcast' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Create' }));
    expect(await screen.findByRole('button', { name: 'Broadcast' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    expect(screen.getByRole('textbox', { name: 'Graph name' })).toHaveValue('Broadcast');
    expect(screen.getByRole('region', { name: 'Routing workspace' })).toBeInTheDocument();
  });

  it('keeps a connected workspace when the virtual-device component is missing and opens its installer', async () => {
    await i18n.changeLanguage('en');
    const driver: DriverPort = {
      getSnapshot: vi.fn().mockResolvedValue({
        ...readyDriver,
        state: 'not_installed',
        installedVersion: null,
        settings: { ...readyDriver.settings, devices: [], canCreate: false },
      }),
      mutate: vi.fn(),
    };
    const url = 'https://downloads.example/MixRelay-Driver.pkg';
    const updates: UpdatesPort = {
      check: vi.fn().mockResolvedValue([
        {
          product: 'mix-relay',
          installedVersion: null,
          latestVersion: '0.1.0',
          state: 'published',
          downloads: [{ os: 'macos', arch: 'universal', url, sha256: 'a'.repeat(64), size: 100 }],
          notesUrl: null,
        },
      ]),
      openDownload: vi.fn().mockResolvedValue(undefined),
    };
    render(
      <MixRelayProvider>
        <I18nProvider>
          <App controlPort={controlPort} driverPort={driver} updatesPort={updates} />
        </I18nProvider>
      </MixRelayProvider>,
    );
    const install = await screen.findByRole('button', { name: 'Get MixRelay installer' });
    expect(screen.getByText('Virtual audio devices are unavailable.')).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Routing workspace' })).toBeInTheDocument();
    expect(driver.mutate).not.toHaveBeenCalled();
    expect(updates.openDownload).not.toHaveBeenCalled();
    fireEvent.click(install);
    const dialog = await screen.findByRole('dialog', { name: 'Updates' });
    expect(within(dialog).getByRole('heading', { name: /^MixRelay$/ })).toBeInTheDocument();
    const download = await within(dialog).findByRole('button', {
      name: 'Open PKG · universal download',
    });
    await waitFor(() => expect(download).toBeEnabled());
    fireEvent.click(download);
    await waitFor(() => expect(updates.openDownload).toHaveBeenCalledWith('mix-relay', url));
  });
});

describe('App platform states', () => {
  it('resolves supported locales and returns mock control status', async () => {
    expect(resolveLocale('ko-KR')).toBe('ko-KR');
    expect(resolveLocale('en-US')).toBe('en');

    await expect(controlPort.getStatus()).resolves.toBe(status);
    await expect(controlPort.getGraph()).resolves.toBe(graph);
    await expect(controlPort.getGraphStatus()).resolves.toMatchObject({ activeRevision: 1 });
    await expect(controlPort.applyGraph({ ...graph, revision: 2 })).resolves.toMatchObject({
      revision: 2,
      state: 'active',
    });
  });

  it('shows unknown health rather than inventing installation state when status is unavailable', async () => {
    const unavailablePort = controlPortWithStatus(
      controlPort,
      vi.fn().mockRejectedValue(new Error('offline')),
    );
    const failedView = render(
      <MixRelayProvider>
        <I18nProvider>
          <App controlPort={unavailablePort} />
        </I18nProvider>
      </MixRelayProvider>,
    );
    await act(async () => Promise.resolve());
    expect(screen.getByRole('button', { name: 'Audio status unknown' })).toBeDisabled();
    expect(screen.queryByText('Not installed')).not.toBeInTheDocument();
    failedView.unmount();

    let finishStatus!: (value: MixRelayStatus) => void;
    const pendingStatus = new Promise<MixRelayStatus>((resolve) => {
      finishStatus = resolve;
    });
    const pendingView = render(
      <MixRelayProvider>
        <I18nProvider>
          <App controlPort={controlPortWithStatus(controlPort, () => pendingStatus)} />
        </I18nProvider>
      </MixRelayProvider>,
    );
    pendingView.unmount();
    await act(async () => finishStatus({ ...status, driverState: 'ready' }));
  });
});

it('keeps product information separate from the confirmed global audio pause control', async () => {
  await i18n.changeLanguage('en');
  const nativeStatus: MixRelayStatus = {
    bufferFrames: null,
    driverState: 'ready',
    overruns: 0,
    protocolVersion: 1,
    sampleRate: null,
    underruns: 0,
    enginePower: { enabled: true, transitioning: false },
  };
  const stopEngine = vi.fn(async (enabled: boolean) => {
    nativeStatus.enginePower = { enabled, transitioning: false };
    return { protocolVersion: 1 as const, revision: 1, state: 'active' as const };
  });
  const controlPort = Object.assign(
    new StorybookMockControlPort({
      graph: { edges: [], nodes: [], revision: 1, schemaVersion: 2 },
      graphStatus: { activeRevision: 1, protocolVersion: 1, submittedRevision: 1 },
      status: nativeStatus,
    }),
    { setEnginePower: stopEngine },
  );
  render(
    <MixRelayProvider>
      <I18nProvider>
        <App controlPort={controlPort} />
      </I18nProvider>
    </MixRelayProvider>,
  );
  expect(await screen.findByRole('button', { name: 'About MixRelay' })).toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'MixRelay' })).toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole('button', { name: 'New graph' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'About MixRelay' }));
  expect(await screen.findByRole('dialog', { name: 'About MixRelay' })).toBeInTheDocument();
  const aboutDialog = screen.getByRole('dialog', { name: 'About MixRelay' });
  expect(within(aboutDialog).getByText('Terms of use')).toBeVisible();
  expect(within(aboutDialog).queryByRole('button', { name: 'Stop audio' })).not.toBeInTheDocument();
  expect(stopEngine).not.toHaveBeenCalled();
  fireEvent.click(within(aboutDialog).getByRole('button', { name: 'Close' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Pause' }));
  await waitFor(() => expect(stopEngine).toHaveBeenCalledWith(false));
  expect(stopEngine).toHaveBeenCalledTimes(1);
  expect(await screen.findByRole('button', { name: 'Resume' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
});

it('keeps file actions in the graph header and the graph list fixed in a wide window', async () => {
  await i18n.changeLanguage('en');
  const controlPort = new StorybookMockControlPort({
    graph: { edges: [], nodes: [], revision: 1, schemaVersion: 2 },
    graphStatus: { activeRevision: 1, protocolVersion: 1, submittedRevision: 1 },
    status: {
      bufferFrames: null,
      driverState: 'not_installed',
      overruns: 0,
      protocolVersion: 1,
      sampleRate: null,
      underruns: 0,
    },
  });
  const workspaceFile = vi.fn().mockResolvedValue(null);
  Object.assign(controlPort, { workspaceFile });
  render(
    <MixRelayProvider>
      <I18nProvider>
        <App controlPort={controlPort} />
      </I18nProvider>
    </MixRelayProvider>,
  );
  await waitFor(() => expect(screen.queryByText('Loading engine')).not.toBeInTheDocument());
  expect(screen.queryByText('Active')).not.toBeInTheDocument();
  const name = screen.getByRole('textbox', { name: 'Graph name' });
  const file = screen.getByRole('button', { name: 'File' });
  expect(file.closest('header')).toBe(name.closest('header'));
  const sidebar = screen.getByRole('complementary', { name: 'Graphs' });
  expect(within(sidebar).getByRole('button', { name: 'New graph' })).toBeInTheDocument();
  // Carbon UI shell: a wide window keeps the list fixed with no on-screen toggle.
  expect(screen.queryByRole('button', { name: 'Graphs' })).not.toBeInTheDocument();
  expect(sidebar).not.toHaveAttribute('hidden');
  expect(screen.getByRole('heading', { name: 'MixRelay' })).toBeVisible();
  expect(screen.getByRole('textbox', { name: 'Graph name' })).toBe(name);
  expect(workspaceFile).not.toHaveBeenCalled();
  fireEvent.click(file);
  // jsdom has no anchor geometry, so Floating UI may hide the mounted menu.
  const menu = await screen.findByRole('menu', { hidden: true });
  expect(within(menu).getByText('Open…')).toBeInTheDocument();
  expect(within(menu).getByText('Save')).toBeInTheDocument();
});
