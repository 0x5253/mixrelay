import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { ControlPortError } from '../../core/domain/control/errors';
import type { GraphApplyResponse, RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { AudioSessionStatus } from '../../core/domain/control/status';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import * as inlineSelectModule from '../../core/ui/InlineSelect';
import { AudioSessionSettings } from './AudioSessionSettings';

const session: AudioSessionStatus = {
  format: { sampleRate: 48000, blockFrames: 128 },
  clockAuthority: 'engine_timer',
  observedDeviceSampleRate: null,
  issue: null,
  transportLatencyFrames: null,
  capabilities: {
    backend: 'contract_only',
    supportedSampleRates: [48000],
    supportedBlockFrames: [128],
    canReconfigureLive: true,
    hardwareInput: false,
    hardwareOutput: false,
    virtualEndpoint: true,
    applicationCapture: false,
    sampleRateConversion: false,
    driftCorrection: false,
  },
};
const graph: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function port() {
  return Object.assign(
    new StorybookMockControlPort({
      status: {
        protocolVersion: 1,
        driverState: 'ready',
        sampleRate: 48000,
        bufferFrames: 128,
        underruns: 0,
        overruns: 0,
      },
      graph,
      graphStatus: { protocolVersion: 1, activeRevision: 2, submittedRevision: 2 },
    }),
    {
      reconfigureSession: vi.fn(
        async (): Promise<GraphApplyResponse> => ({
          protocolVersion: 1,
          revision: 2,
          state: 'active',
        }),
      ),
    },
  );
}

function view(
  controlPort: ReturnType<typeof port>,
  onApplied = vi.fn(),
  onBusyChange = vi.fn(),
  sessionStatus = session,
  disabled = false,
) {
  return (
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <AudioSessionSettings
          disabled={disabled}
          controlPort={controlPort}
          session={sessionStatus}
          onApplied={onApplied}
          onBusyChange={onBusyChange}
        />
      </MixRelayProvider>
    </I18nextProvider>
  );
}

async function chooseBuffer(value = '128') {
  if (!screen.queryByRole('listbox')) {
    const input = screen.getByRole('button', { name: /Processing buffer$/ });
    if (input.hasAttribute('disabled')) return;
    fireEvent.click(input);
  }
  const list = await screen.findByRole('listbox');
  fireEvent.click(within(list).getByRole('option', { name: value }));
}

it('stacks native and request errors and allows retry of the same single supported buffer', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  controlPort.reconfigureSession.mockRejectedValueOnce(new Error('Failed'));
  render(
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <AudioSessionSettings
          controlPort={controlPort}
          session={{ ...session, issue: 'stored_format_unavailable' }}
          sessionAlerts={['Native format failed', 'Preferences unavailable']}
          onApplied={vi.fn()}
          onBusyChange={vi.fn()}
        />
      </MixRelayProvider>
    </I18nextProvider>,
  );
  await chooseBuffer();
  await waitFor(() =>
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('audioSession.changeFailed')),
  );
  expect(screen.getAllByRole('alert')).toHaveLength(1);
  expect(screen.getByRole('alert')).toHaveTextContent('Native format failed');
  expect(screen.getByRole('alert')).toHaveTextContent('Preferences unavailable');
  fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
  await waitFor(() => expect(controlPort.reconfigureSession).toHaveBeenCalledTimes(2));
  expect(controlPort.reconfigureSession).toHaveBeenLastCalledWith({
    expectedRevision: 1,
    format: session.format,
  });
});

it('blocks buffer writes, including a delivered selector callback, while graph edits are uncommitted', async () => {
  const controlPort = port();
  const onApplied = vi.fn();
  const onBusyChange = vi.fn();
  const selectorSpy = vi.spyOn(inlineSelectModule, 'InlineSelect');
  try {
    render(view(controlPort, onApplied, onBusyChange, session, true));
    expect(screen.getByRole('button')).toBeDisabled();
    const selectorProps = selectorSpy.mock.calls.at(-1)?.[0];
    expect(selectorProps?.disabled).toBe(true);
    if (!selectorProps) throw new Error('InlineSelect was not rendered');
    await act(async () => selectorProps.onChange('128'));
    expect(controlPort.reconfigureSession).not.toHaveBeenCalled();
    expect(onApplied).not.toHaveBeenCalled();
    expect(onBusyChange).not.toHaveBeenCalled();
  } finally {
    selectorSpy.mockRestore();
  }
});

it('submits advertised processing buffer 32 without changing the clock-owned rate', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  const editable = {
    ...session,
    capabilities: { ...session.capabilities, supportedBlockFrames: [32, 64, 128, 256] },
  };
  render(view(controlPort, vi.fn(), vi.fn(), editable));
  fireEvent.click(screen.getByRole('button', { name: /Processing buffer$/ }));
  await chooseBuffer('32');
  expect(screen.getByRole('button', { name: /Processing buffer$/ })).toHaveTextContent('32');
  await waitFor(() =>
    expect(controlPort.reconfigureSession).toHaveBeenCalledWith({
      expectedRevision: graph.revision,
      format: { sampleRate: session.format.sampleRate, blockFrames: 32 },
    }),
  );
});

it('exposes all advertised buffers without offering an independent graph sample rate', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  const rates = [44100, 48000, 88200, 96000, 176400, 192000];
  const buffers = [32, 64, 128, 256, 512, 1024, 2048];
  render(
    view(controlPort, vi.fn(), vi.fn(), {
      ...session,
      format: { sampleRate: 192000, blockFrames: 128 },
      capabilities: {
        ...session.capabilities,
        supportedSampleRates: rates,
        supportedBlockFrames: buffers,
      },
    }),
  );
  expect(screen.queryByRole('button', { name: /Sample rate$/ })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: /Processing buffer$/ }));
  for (const buffer of buffers)
    expect(screen.getByRole('option', { name: String(buffer) })).toBeInTheDocument();
  await chooseBuffer('2048');
  await waitFor(() =>
    expect(controlPort.reconfigureSession).toHaveBeenCalledWith({
      expectedRevision: graph.revision,
      format: { sampleRate: 192000, blockFrames: 2048 },
    }),
  );
});

it('does not submit stale choices while native format discovery is unavailable', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  const unavailable = {
    ...session,
    capabilities: { ...session.capabilities, supportedBlockFrames: [] },
  };
  const { rerender } = render(view(controlPort, vi.fn(), vi.fn(), unavailable));
  expect(screen.getByRole('button')).toBeDisabled();
  await chooseBuffer();
  expect(controlPort.reconfigureSession).not.toHaveBeenCalled();
  rerender(view(controlPort));
  expect(screen.getByRole('button')).toBeEnabled();
  await chooseBuffer();
  await waitFor(() => expect(controlPort.reconfigureSession).toHaveBeenCalledOnce());
});

it('does not deliver an old port final document or clear the new owner busy state', async () => {
  await i18n.changeLanguage('en');
  const oldPort = port();
  const nextPort = port();
  const oldRead = deferred<RuntimeGraphDocument>();
  const nextRead = deferred<RuntimeGraphDocument>();
  vi.spyOn(oldPort, 'getGraph').mockResolvedValueOnce(graph).mockReturnValueOnce(oldRead.promise);
  vi.spyOn(nextPort, 'getGraph').mockReturnValueOnce(nextRead.promise);
  const onApplied = vi.fn();
  const onBusyChange = vi.fn();
  const rendered = render(view(oldPort, onApplied, onBusyChange));
  await chooseBuffer();
  await waitFor(() => expect(oldPort.getGraph).toHaveBeenCalledTimes(2));
  rendered.rerender(view(nextPort, onApplied, onBusyChange));
  // jsdom has no layout bounds after a disabled-to-enabled rerender, so Floating UI
  // hides the portal even though the Combobox is expanded. Exercise this port race
  // through the same option submission without weakening its ownership assertions.
  await chooseBuffer('128');
  await waitFor(() => expect(nextPort.getGraph).toHaveBeenCalledTimes(1));
  const busyCalls = onBusyChange.mock.calls.length;
  await act(async () => oldRead.resolve({ ...graph, revision: 100 }));
  expect(onApplied).not.toHaveBeenCalled();
  expect(onBusyChange).toHaveBeenCalledTimes(busyCalls);
  expect(screen.getByRole('button')).toBeDisabled();
  rendered.unmount();
  await act(async () => nextRead.resolve(graph));
  expect(nextPort.reconfigureSession).not.toHaveBeenCalled();
});

it('does not issue a native reconfiguration after unmounting during the initial read', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  const read = deferred<RuntimeGraphDocument>();
  vi.spyOn(controlPort, 'getGraph').mockReturnValue(read.promise);
  const onApplied = vi.fn();
  const onBusyChange = vi.fn();
  const rendered = render(view(controlPort, onApplied, onBusyChange));
  await chooseBuffer();
  rendered.unmount();
  const busyCalls = onBusyChange.mock.calls.length;
  await act(async () => read.resolve(graph));
  expect(controlPort.reconfigureSession).not.toHaveBeenCalled();
  expect(onApplied).not.toHaveBeenCalled();
  expect(onBusyChange).toHaveBeenCalledTimes(busyCalls);
});

it('ignores a rejected old request after replacing the port', async () => {
  await i18n.changeLanguage('en');
  const oldPort = port();
  const read = deferred<RuntimeGraphDocument>();
  vi.spyOn(oldPort, 'getGraph').mockReturnValue(read.promise);
  const onBusyChange = vi.fn();
  const rendered = render(view(oldPort, vi.fn(), onBusyChange));
  await chooseBuffer();
  rendered.rerender(view(port(), vi.fn(), onBusyChange));
  const busyCalls = onBusyChange.mock.calls.length;
  await act(async () => read.reject(new Error('old connection failed')));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(onBusyChange).toHaveBeenCalledTimes(busyCalls);
  expect(screen.getByRole('button')).toBeEnabled();
});

it.each(['reconfiguration', 'activation'] as const)(
  'stops the old continuation after unmounting during %s',
  async (stage) => {
    await i18n.changeLanguage('en');
    const controlPort = port();
    const pending = deferred<{
      protocolVersion: 1;
      revision: number;
      state: 'active';
      activeRevision: number;
      submittedRevision: number;
    }>();
    vi.spyOn(controlPort, 'getGraph');
    const status = vi.spyOn(controlPort, 'getGraphStatus');
    if (stage === 'reconfiguration')
      controlPort.reconfigureSession.mockReturnValueOnce(pending.promise);
    else status.mockReturnValueOnce(pending.promise);
    const onApplied = vi.fn();
    const onBusyChange = vi.fn();
    const rendered = render(view(controlPort, onApplied, onBusyChange));
    await chooseBuffer();
    await waitFor(() =>
      expect(stage === 'activation' ? status : controlPort.reconfigureSession).toHaveBeenCalled(),
    );
    rendered.unmount();
    const busyCalls = onBusyChange.mock.calls.length;
    await act(async () =>
      pending.resolve({
        protocolVersion: 1,
        revision: 2,
        state: 'active',
        activeRevision: 2,
        submittedRevision: 2,
      }),
    );
    expect(controlPort.getGraph).toHaveBeenCalledTimes(1);
    expect(status).toHaveBeenCalledTimes(stage === 'activation' ? 1 : 0);
    expect(onApplied).not.toHaveBeenCalled();
    expect(onBusyChange).toHaveBeenCalledTimes(busyCalls);
  },
);

it('still publishes a current owner session exactly once', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  const next = { ...graph, revision: 2 };
  vi.spyOn(controlPort, 'getGraph').mockResolvedValueOnce(graph).mockResolvedValueOnce(next);
  const onApplied = vi.fn();
  const onBusyChange = vi.fn();
  render(view(controlPort, onApplied, onBusyChange));
  await chooseBuffer();
  await chooseBuffer();
  await waitFor(() => expect(onApplied).toHaveBeenCalledWith(next));
  expect(onApplied).toHaveBeenCalledTimes(1);
  expect(controlPort.reconfigureSession).toHaveBeenCalledTimes(1);
  expect(onBusyChange.mock.calls).toEqual([[true], [false]]);
});

it('follows external format changes in untouched fields without sending a native write', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  const rendered = render(view(controlPort));
  const nextSession: AudioSessionStatus = {
    ...session,
    format: { sampleRate: 44100, blockFrames: 256 },
    capabilities: {
      ...session.capabilities,
      supportedSampleRates: [44100, 48000],
      supportedBlockFrames: [128, 256],
    },
  };
  rendered.rerender(view(controlPort, vi.fn(), vi.fn(), nextSession));
  expect(screen.queryByRole('button', { name: /Sample rate$/ })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: /Processing buffer$/ })).toHaveTextContent('256');
  expect(controlPort.reconfigureSession).not.toHaveBeenCalled();
  rendered.rerender(view(port(), vi.fn(), vi.fn(), nextSession));
  expect(screen.getByRole('button', { name: /Processing buffer$/ })).toHaveTextContent('256');
});

it('uses the current clock rate for a selection after an external format change', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  const changed = {
    ...session,
    format: { sampleRate: 44100, blockFrames: 512 },
    capabilities: { ...session.capabilities, supportedBlockFrames: [128, 256, 512] },
  };
  const rendered = render(view(controlPort));
  rendered.rerender(view(controlPort, vi.fn(), vi.fn(), changed));
  await chooseBuffer('256');
  await waitFor(() =>
    expect(controlPort.reconfigureSession).toHaveBeenCalledExactlyOnceWith({
      expectedRevision: graph.revision,
      format: { sampleRate: 44100, blockFrames: 256 },
    }),
  );
  // An unchanged stale status poll must not flash back to the old buffer.
  rendered.rerender(view(controlPort, vi.fn(), vi.fn(), { ...changed }));
  expect(screen.getByRole('button')).toHaveTextContent('256');
  rendered.rerender(view(port(), vi.fn(), vi.fn(), changed));
  expect(screen.getByRole('button')).toHaveTextContent('512');
});

it('defers external field updates during apply and adopts them when the request finishes', async () => {
  await i18n.changeLanguage('en');
  const controlPort = port();
  const pending = deferred<GraphApplyResponse>();
  controlPort.reconfigureSession.mockReturnValueOnce(pending.promise);
  const editable = {
    ...session,
    capabilities: {
      ...session.capabilities,
      supportedSampleRates: [44100, 48000],
      supportedBlockFrames: [128, 256],
    },
  };
  const applied = vi.fn();
  const busy = vi.fn();
  const rendered = render(view(controlPort, applied, busy, editable));
  await chooseBuffer();
  await waitFor(() => expect(controlPort.reconfigureSession).toHaveBeenCalledTimes(1));
  rendered.rerender(
    view(controlPort, applied, busy, {
      ...editable,
      format: { sampleRate: 44100, blockFrames: 256 },
    }),
  );
  expect(screen.getByRole('button', { name: /Processing buffer$/ })).toHaveTextContent('128');
  expect(screen.getByRole('button', { name: /Processing buffer$/ })).toBeDisabled();
  await act(async () => pending.resolve({ protocolVersion: 1, revision: 2, state: 'active' }));
  expect(screen.getByRole('button', { name: /Processing buffer$/ })).toHaveTextContent('256');
  expect(controlPort.reconfigureSession).toHaveBeenCalledTimes(1);
  expect(controlPort.reconfigureSession).toHaveBeenCalledWith({
    expectedRevision: graph.revision,
    format: session.format,
  });
});

it('keeps readback unchanged and releases its lease when native reconfiguration reports the existing revision', async () => {
  await i18n.changeLanguage('en');
  const source = port();
  source.reconfigureSession.mockResolvedValue({
    protocolVersion: 1,
    revision: graph.revision,
    state: 'active',
  });
  const read = vi.spyOn(source, 'getGraph');
  const applied = vi.fn();
  const busy = vi.fn();
  render(view(source, applied, busy));
  await chooseBuffer();
  await waitFor(() => expect(busy).toHaveBeenLastCalledWith(false));
  expect(read).toHaveBeenCalledOnce();
  expect(applied).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: /Processing buffer$/ })).toHaveTextContent('128');
});

it('shows the native session rejection code without replacing its readback', async () => {
  await i18n.changeLanguage('en');
  const source = port();
  source.reconfigureSession.mockRejectedValue(
    new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} }),
  );
  render(view(source));
  await chooseBuffer();
  await waitFor(() => expect(screen.getByRole('alert')).toBeVisible());
  expect(screen.getByRole('alert')).toHaveTextContent(
    i18n.t('audioSession.errors.graph_revision_stale'),
  );
  expect(screen.getByRole('button', { name: /Processing buffer$/ })).toHaveTextContent('128');
});
