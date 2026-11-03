import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { MixRelayStatus } from '../../core/domain/control/status';
import { i18n } from '../../core/i18n';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioPauseControl } from './AudioPauseControl';

function fixture() {
  return new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 256,
      underruns: 0,
      overruns: 0,
    },
  });
}

it('switches through control IPC and waits for native acknowledgement without changing the graph', async () => {
  await i18n.changeLanguage('en');
  const port = fixture();
  const status = {
    ...(await port.getStatus()),
    enginePower: { enabled: true, transitioning: false },
  };
  const graph = await port.getGraph();
  const switchPower = vi.fn().mockResolvedValue({ revision: graph.revision });
  const controlPort = Object.assign(port, { setEnginePower: switchPower });
  vi.spyOn(port, 'getStatus').mockResolvedValue({
    ...status,
    enginePower: { enabled: false, transitioning: false },
  });
  const onApplied = vi.fn();
  const onBusyChange = vi.fn();
  render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <AudioPauseControl
          controlPort={controlPort}
          status={status}
          onApplied={onApplied}
          onBusyChange={onBusyChange}
        />
      </I18nextProvider>
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
  expect(screen.getByRole('button', { name: 'Changing audio state…' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Changing audio state…' })).toHaveTextContent('');
  await waitFor(() => expect(onApplied).toHaveBeenCalledWith(graph));
  expect(switchPower).toHaveBeenCalledExactlyOnceWith(false);
  expect(onBusyChange.mock.calls).toEqual([[true], [false]]);
  expect(screen.getByRole('button', { name: 'Resume' })).toHaveAttribute('aria-pressed', 'true');
});

it('shows failure without inventing an off state', async () => {
  await i18n.changeLanguage('en');
  const port = fixture();
  const status = {
    ...(await port.getStatus()),
    enginePower: { enabled: true, transitioning: false },
  };
  const controlPort = Object.assign(port, {
    setEnginePower: vi.fn().mockRejectedValue(new Error('unavailable')),
  });
  render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <AudioPauseControl
          controlPort={controlPort}
          status={status}
          onApplied={vi.fn()}
          onBusyChange={vi.fn()}
        />
      </I18nextProvider>
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
  await waitFor(() => expect(screen.getByRole('alert')).toBeVisible());
  expect(screen.getByRole('button', { name: 'Pause' })).toHaveAttribute('aria-pressed', 'false');
});

it('warns while the service reports the state unsaved and clears when its retry saves it', async () => {
  await i18n.changeLanguage('en');
  const port = fixture();
  const running = {
    ...(await port.getStatus()),
    enginePower: { enabled: true, transitioning: false },
  };
  const graph = await port.getGraph();
  const controlPort = Object.assign(port, {
    setEnginePower: vi.fn().mockResolvedValue({ revision: graph.revision }),
  });
  vi.spyOn(port, 'getStatus').mockResolvedValue({
    ...running,
    enginePower: { enabled: false, transitioning: false, unsaved: true },
  });
  const onApplied = vi.fn();
  const view = (status: MixRelayStatus) => (
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <AudioPauseControl
          controlPort={controlPort}
          status={status}
          onApplied={onApplied}
          onBusyChange={vi.fn()}
        />
      </I18nextProvider>
    </MixRelayProvider>
  );
  const { rerender } = render(view(running));
  fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
  await waitFor(() => expect(onApplied).toHaveBeenCalledWith(graph));
  // The change stands; only its durability is in doubt, so this is not an alert.
  expect(screen.getByRole('status')).toHaveTextContent(
    'This change was applied but could not be saved',
  );
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.getByRole('button', { name: 'Resume' })).toHaveAttribute('aria-pressed', 'true');
  rerender(view({ ...running, enginePower: { enabled: false, transitioning: false } }));
  expect(screen.queryByRole('status')).toBeNull();
});

it('follows Driver menu changes without sending another toggle', async () => {
  await i18n.changeLanguage('en');
  const port = fixture();
  const change = vi.fn();
  const controlPort = Object.assign(port, { setEnginePower: change });
  const status = await port.getStatus();
  const renderState = (enabled: boolean, transitioning: boolean) => (
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <AudioPauseControl
          controlPort={controlPort}
          status={{ ...status, enginePower: { enabled, transitioning } }}
          onApplied={vi.fn()}
          onBusyChange={vi.fn()}
        />
      </I18nextProvider>
    </MixRelayProvider>
  );
  const view = render(renderState(true, false));
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  view.rerender(renderState(false, true));
  expect(screen.getByRole('button', { name: 'Changing audio state…' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Changing audio state…' })).toHaveTextContent('');
  view.rerender(renderState(false, false));
  expect(screen.getByRole('button', { name: 'Resume' })).toHaveAttribute('aria-pressed', 'true');
  view.rerender(renderState(true, false));
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  expect(change).not.toHaveBeenCalled();
});

function deferredPause<T>() {
  let resolve = (_value: T) => {};
  let reject = (_cause: unknown) => {};
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function pauseView(
  controlPort: ControlPort,
  status: MixRelayStatus,
  applied = vi.fn(),
  busy = vi.fn(),
) {
  return (
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <AudioPauseControl
          controlPort={controlPort}
          status={status}
          onApplied={applied}
          onBusyChange={busy}
        />
      </MixRelayProvider>
    </I18nextProvider>
  );
}

it.each(['power', 'graph', 'readback'] as const)(
  'does not publish a late %s acknowledgement after its view unmounts',
  async (stage) => {
    await i18n.changeLanguage('en');
    const source = fixture();
    const status = {
      ...(await source.getStatus()),
      enginePower: { enabled: true, transitioning: false },
    };
    const power = deferredPause<{ revision: number }>();
    const graph = deferredPause<RuntimeGraphDocument>();
    const readback = deferredPause<MixRelayStatus>();
    const toggle = vi
      .fn()
      .mockReturnValue(stage === 'power' ? power.promise : Promise.resolve({ revision: 1 }));
    const port = Object.assign(source, { setEnginePower: toggle });
    const read = vi.spyOn(source, 'getGraph');
    if (stage === 'graph') read.mockReturnValue(graph.promise);
    const observe = vi.spyOn(source, 'getStatus');
    if (stage === 'readback') observe.mockReturnValue(readback.promise);
    const applied = vi.fn();
    const busy = vi.fn();
    const view = render(pauseView(port, status, applied, busy));
    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await waitFor(() =>
      expect(
        stage === 'power' ? toggle : stage === 'graph' ? read : observe,
      ).toHaveBeenCalledOnce(),
    );
    view.unmount();
    await act(async () => {
      power.resolve({ revision: 1 });
      graph.resolve({ schemaVersion: 2, revision: 1, nodes: [], edges: [] });
      readback.resolve({ ...status, enginePower: { enabled: false, transitioning: false } });
    });
    expect(applied).not.toHaveBeenCalled();
    expect(busy.mock.calls).toEqual([[true], [false]]);
  },
);

it('ignores a rejected power request from the previous service owner', async () => {
  await i18n.changeLanguage('en');
  const source = fixture();
  const status = {
    ...(await source.getStatus()),
    enginePower: { enabled: true, transitioning: false },
  };
  const pending = deferredPause<{ revision: number }>();
  const old = Object.assign(source, { setEnginePower: vi.fn().mockReturnValue(pending.promise) });
  const applied = vi.fn();
  const view = render(pauseView(old, status, applied));
  fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
  const replacement = Object.assign(fixture(), { setEnginePower: vi.fn() });
  view.rerender(pauseView(replacement, status, applied));
  await act(async () => pending.reject(new Error('Old service stopped')));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  expect(applied).not.toHaveBeenCalled();
});

it('sends one power transition when two activations arrive in one render batch', async () => {
  await i18n.changeLanguage('en');
  const source = fixture();
  const status = {
    ...(await source.getStatus()),
    enginePower: { enabled: true, transitioning: false },
  };
  const pending = deferredPause<{ revision: number }>();
  const toggle = vi.fn().mockReturnValue(pending.promise);
  const busy = vi.fn();
  const view = render(
    pauseView(Object.assign(source, { setEnginePower: toggle }), status, vi.fn(), busy),
  );
  const button = screen.getByRole('button', { name: 'Pause' });
  act(() => {
    fireEvent.click(button);
    fireEvent.click(button);
  });
  expect(toggle).toHaveBeenCalledExactlyOnceWith(false);
  expect(busy).toHaveBeenCalledExactlyOnceWith(true);
  view.unmount();
  await act(async () => pending.resolve({ revision: 1 }));
});

it('keeps unavailable native playback state or command capability from accepting a pause action', async () => {
  await i18n.changeLanguage('en');
  const port = fixture();
  const graphRead = vi.spyOn(port, 'getGraph');
  const toggle = vi.fn();
  const status = await port.getStatus();
  const view = render(pauseView(Object.assign(port, { setEnginePower: toggle }), status));
  expect(screen.getByRole('button')).toBeDisabled();
  fireEvent.click(screen.getByRole('button'));
  expect(toggle).not.toHaveBeenCalled();
  view.rerender(
    pauseView(fixture(), { ...status, enginePower: { enabled: true, transitioning: false } }),
  );
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
  expect(graphRead).not.toHaveBeenCalled();
});

it('presents the healthy running transport with its neutral action tone and native status description', async () => {
  await i18n.changeLanguage('en');
  const source = fixture();
  const status: MixRelayStatus = {
    ...(await source.getStatus()),
    enginePower: { enabled: true, transitioning: false },
    audioSession: {
      format: { sampleRate: 48000, blockFrames: 128 },
      clockAuthority: 'engine_timer',
      processingState: 'running',
      observedDeviceSampleRate: null,
      transportLatencyFrames: null,
      issue: null,
      capabilities: {
        backend: 'core_audio',
        supportedSampleRates: [48000],
        supportedBlockFrames: [128],
        canReconfigureLive: true,
        hardwareInput: true,
        hardwareOutput: true,
        virtualEndpoint: true,
        applicationCapture: true,
        sampleRateConversion: true,
        driftCorrection: true,
      },
    },
  };
  render(pauseView(Object.assign(source, { setEnginePower: vi.fn() }), status));
  const pause = screen.getByRole('button', { name: 'Pause' });
  expect(pause).toHaveAttribute('data-tone', 'neutral');
  expect(pause).toHaveAttribute('aria-description', i18n.t('routing.nodeStatus.running'));
  expect(pause).toBeEnabled();
});

it('explains a pause the engine chose after repeated failures until Resume', async () => {
  await i18n.changeLanguage('en');
  const port = fixture();
  const status = {
    ...(await port.getStatus()),
    enginePower: {
      enabled: false,
      transitioning: false,
      unsaved: true,
      pauseReason: 'repeatedFailures' as const,
    },
  };
  const view = (current: MixRelayStatus) => (
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <AudioPauseControl
          controlPort={port}
          status={current}
          onApplied={vi.fn()}
          onBusyChange={vi.fn()}
        />
      </I18nextProvider>
    </MixRelayProvider>
  );
  const { rerender } = render(view(status));
  // The reason for the pause outranks the save warning; neither is an alert.
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('audioSession.pausedAfterFailures'));
  expect(screen.queryByRole('alert')).toBeNull();
  rerender(view({ ...status, enginePower: { enabled: true, transitioning: false } }));
  expect(screen.queryByRole('status')).toBeNull();
});
