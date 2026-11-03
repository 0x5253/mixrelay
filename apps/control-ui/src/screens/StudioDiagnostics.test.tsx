import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { processorReportState } from '../../.storybook/fixtures/processorReport';
import type { MixRelayStatus } from '../core/domain/control/status';
import { I18nProvider } from '../core/i18n';
import type { DiagnosticsPort } from '../core/ports/DiagnosticsPort';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import { readyDriver } from '../features/driver-status/driverFixtures';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import type { useProcessorCatalog } from '../features/routing-workspace/useProcessorCatalog';
import {
  noWorkspaceProblems,
  type WorkspaceProblemSummary,
} from '../features/routing-workspace/WorkspaceProblems';
import { eventually, finishFakeTimerTest } from '../test-support/fakeTimerQueries';
import { StudioDiagnostics } from './StudioDiagnostics';

const baseStatus: MixRelayStatus = {
  ...processorReportState().status,
  enginePower: { enabled: true, transitioning: false },
  audioSession: {
    processingState: 'running',
    issue: null,
    format: { sampleRate: 48000, blockFrames: 128 },
    clockAuthority: 'engine_timer',
    observedDeviceSampleRate: null,
    transportLatencyFrames: null,
    capabilities: {
      backend: 'core_audio',
      supportedSampleRates: [48000],
      supportedBlockFrames: [128],
      canReconfigureLive: false,
      hardwareInput: true,
      hardwareOutput: true,
      virtualEndpoint: true,
      applicationCapture: true,
      sampleRateConversion: false,
      driftCorrection: false,
    },
  },
};
function catalog(): ReturnType<typeof useProcessorCatalog> {
  return {
    processors: [],
    diagnostics: [],
    busy: false,
    failure: null,
    refresh: vi.fn(),
    retry: vi.fn(),
  };
}
function view(
  status: MixRelayStatus | null,
  search = catalog(),
  driverSession?: DriverSession,
  options: {
    problems?: WorkspaceProblemSummary;
    problemsRef?: (element: HTMLDivElement | null) => void;
    diagnosticsPort?: DiagnosticsPort;
  } = {},
) {
  return (
    <MixRelayProvider>
      <I18nProvider>
        <StudioDiagnostics
          status={status}
          catalog={search}
          driverSession={driverSession}
          problems={options.problems ?? noWorkspaceProblems}
          problemsRef={options.problemsRef ?? (() => undefined)}
          diagnosticsPort={options.diagnosticsPort}
        />
      </I18nProvider>
    </MixRelayProvider>
  );
}

afterEach(() => finishFakeTimerTest(() => act(async () => vi.runOnlyPendingTimersAsync())));

it('opens diagnostics by keyboard without scanning and returns focus when dismissed', async () => {
  vi.useFakeTimers();
  const search = catalog();
  const diagnostic = {
    format: 'vst3',
    stage: 'probe',
    pluginName: 'Unavailable effect',
    message: 'Native process exited',
  } as const;
  search.diagnostics = [diagnostic, diagnostic];
  render(view(baseStatus, search));
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  const trigger = screen.getByRole('button', { name: 'Diagnostics · 1' });
  expect(trigger).toHaveAccessibleDescription('Scan issues (1)');
  trigger.focus();
  await user.keyboard('{Enter}');
  const dialog = screen.getByRole('dialog', { name: 'Diagnostics' });
  expect(within(dialog).getByText('Audio session running')).toBeVisible();
  expect(within(dialog).getByText(/48000 Hz · 128 frames/)).toBeVisible();
  expect(within(dialog).getByRole('heading', { name: /Unavailable effect/ })).toBeVisible();
  expect(search.refresh).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await eventually(() => expect(trigger).toHaveFocus());
  await user.keyboard(' ');
  await user.click(screen.getByRole('button', { name: 'Close' }));
  await eventually(() => expect(trigger).toHaveFocus());
});

it('reports missing observations without showing a healthy engine or driver', async () => {
  vi.useFakeTimers();
  render(view(null));
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  await user.click(screen.getByRole('button', { name: 'Diagnostics' }));
  const dialog = screen.getByRole('dialog', { name: 'Diagnostics' });
  expect(within(dialog).getByText('Audio status unknown')).toBeVisible();
  expect(within(dialog).getByText('Audio device unavailable')).toBeVisible();
  expect(within(dialog).queryByText(/48000/)).not.toBeInTheDocument();
});

it('preserves a confirmed pause, unsaved state and driver errors in the report', async () => {
  vi.useFakeTimers();
  const status = {
    ...baseStatus,
    enginePower: {
      enabled: false,
      transitioning: false,
      unsaved: true,
      pauseReason: 'repeatedFailures' as const,
    },
  };
  const driver: DriverSession = {
    port: null,
    snapshot: { ...readyDriver, state: 'unavailable' },
    busy: false,
    error: 'Driver settings could not be saved',
    endpointId: null,
    refresh: vi.fn(),
    mutate: vi.fn(),
    changeDevice: vi.fn(),
  };
  render(view(status, catalog(), driver));
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  await user.click(screen.getByRole('button', { name: 'Diagnostics' }));
  const dialog = screen.getByRole('dialog', { name: 'Diagnostics' });
  expect(within(dialog).getByText('Audio paused')).toBeVisible();
  expect(within(dialog).getByText(/engine stopped unexpectedly/)).toBeVisible();
  expect(within(dialog).getByText(/may not be kept after MixRelay restarts/)).toBeVisible();
  expect(within(dialog).getByText('Driver settings could not be saved')).toBeVisible();
  expect(within(dialog).queryByText('Audio session running')).not.toBeInTheDocument();
});

it('counts current problems, hosts their recovery and saves a report without names or paths', async () => {
  vi.useFakeTimers();
  const search = catalog();
  search.diagnostics = [
    { format: 'vst3', stage: 'probe', pluginName: 'Private Effect', message: '/Users/name/x.vst3' },
  ];
  const saveReport = vi
    .fn<DiagnosticsPort['saveReport']>()
    .mockRejectedValueOnce(new Error('invoke failed'))
    .mockResolvedValue(undefined);
  let host: HTMLDivElement | null = null;
  render(
    view(baseStatus, search, undefined, {
      problems: { codes: ['graph_apply_timeout'], tone: 'critical' },
      problemsRef: (element) => {
        host = element;
      },
      diagnosticsPort: { saveReport },
    }),
  );
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  const trigger = screen.getByRole('button', { name: 'Diagnostics · 2' });
  expect(screen.getByRole('img', { name: 'Diagnostics: current problems (1)' })).toHaveAttribute(
    'data-tone',
    'critical',
  );
  expect(trigger).toHaveAccessibleDescription(
    'Diagnostics: current problems (1) · Scan issues (1)',
  );
  await user.click(trigger);
  const dialog = screen.getByRole('dialog', { name: 'Diagnostics' });
  const problems = within(dialog).getByRole('region', { name: 'Current problems' });
  expect(host).not.toBeNull();
  expect(problems).toContainElement(host);
  await user.click(within(dialog).getByRole('button', { name: 'Save Diagnostic Report…' }));
  await eventually(() =>
    expect(within(dialog).getByRole('alert')).toHaveTextContent(
      'The diagnostic report could not be started.',
    ),
  );
  await user.click(within(dialog).getByRole('button', { name: 'Save Diagnostic Report…' }));
  await eventually(() => expect(within(dialog).queryByRole('alert')).not.toBeInTheDocument());
  expect(saveReport).toHaveBeenLastCalledWith({
    problems: ['graph_apply_timeout'],
    processorScan: [{ format: 'vst3', stage: 'probe', count: 1 }],
  });
  expect(JSON.stringify(saveReport.mock.calls)).not.toMatch(/Private|Users/u);
});

it('omits the problem section and report action when neither applies', async () => {
  vi.useFakeTimers();
  render(view(baseStatus));
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  await user.click(screen.getByRole('button', { name: 'Diagnostics' }));
  const dialog = screen.getByRole('dialog', { name: 'Diagnostics' });
  expect(within(dialog).queryByRole('region', { name: 'Current problems' })).toBeNull();
  expect(within(dialog).queryByRole('button', { name: /Diagnostic Report/u })).toBeNull();
});
