import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import type { AudioSessionStatus, MixRelayStatus } from '../core/domain/control/status';
import '../test-support/integration';
import { render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { describe, expect, it, vi } from 'vitest';
import { i18n } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import { StudioScreen } from './StudioScreen';

vi.mock('../features/routing-workspace/RoutingWorkspace', () => ({ RoutingWorkspace: () => null }));
vi.mock('../features/audio-session/AudioSessionControl', () => ({
  AudioSessionControl: () => null,
}));

const session: AudioSessionStatus = {
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
};
const healthy: MixRelayStatus = {
  protocolVersion: 1,
  driverState: 'ready',
  sampleRate: 48000,
  bufferFrames: 128,
  underruns: 0,
  overruns: 0,
  audioSession: session,
};
type Case = [string, MixRelayStatus | null, string];
const cases: Case[] = [
  ['healthy', healthy, 'routing.nodeStatus.running'],
  [
    'stopped',
    { ...healthy, audioSession: { ...session, processingState: 'stopped' } },
    'routing.nodeStatus.stopped',
  ],
  [
    'processing failed',
    { ...healthy, audioSession: { ...session, issue: 'processing_failed' } },
    'routing.nodeStatus.failed',
  ],
  [
    'unsynchronized clock',
    { ...healthy, audioSession: { ...session, issue: 'clock_not_synchronized' } },
    'routing.nodeStatus.issues.clock_not_synchronized',
  ],
  [
    'native buffer mismatch',
    {
      ...healthy,
      audioSession: { ...session, observedDeviceBufferFrames: 512, issue: 'block_size_mismatch' },
    },
    'routing.nodeStatus.issues.block_size_mismatch',
  ],
  [
    'missing app',
    { ...healthy, audioSession: { ...session, issue: 'application_unavailable' } },
    'routing.nodeStatus.issues.application_unavailable',
  ],
  [
    'ambiguous app',
    { ...healthy, audioSession: { ...session, issue: 'application_ambiguous' } },
    'routing.nodeStatus.issues.application_ambiguous',
  ],
  [
    'device unavailable',
    { ...healthy, audioSession: { ...session, issue: 'device_unavailable' } },
    'routing.nodeStatus.issues.device_unavailable',
  ],
  [
    'settings warning',
    { ...healthy, audioSession: { ...session, endpointPreferencesUnavailable: true } },
    'routing.nodeStatus.issues.preferences_unavailable',
  ],
  [
    'idle',
    { ...healthy, audioSession: { ...session, processingState: 'idle' } },
    'routing.nodeStatus.idle',
  ],
  [
    'unknown processing state',
    { ...healthy, audioSession: { ...session, processingState: undefined } },
    'routing.nodeStatus.unknown',
  ],
  ['missing session', { ...healthy, audioSession: undefined }, 'routing.nodeStatus.unknown'],
  ['unavailable transport', null, 'routing.nodeStatus.unknown'],
  ['unavailable driver', { ...healthy, driverState: 'unavailable' }, 'driver.states.unavailable'],
  ['blocked driver', { ...healthy, driverState: 'blocked' }, 'driver.states.blocked'],
  ['installing driver', { ...healthy, driverState: 'installing' }, 'driver.states.installing'],
];

function fixture(status: MixRelayStatus | null) {
  const port = new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: status ?? healthy,
  });
  if (!status) vi.spyOn(port, 'getStatus').mockRejectedValue(new Error('offline'));
  return render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <StudioScreen controlPort={port} />
      </I18nextProvider>
    </MixRelayProvider>,
  );
}

describe.each(['en', 'ko-KR', 'pseudo'])('Studio footer health in %s', (locale) => {
  it.each(cases)(
    'describes %s on the disabled engine control when power is unavailable',
    async (_name, status, key) => {
      await i18n.changeLanguage(locale);
      const view = fixture(status);
      const label = i18n.t(key);
      expect(label).not.toBe(key);
      const button = await screen.findByRole('button', { name: label });
      expect(button).toBeDisabled();
      expect(button).toHaveAttribute('aria-pressed', 'false');
      expect(button).toHaveAttribute('aria-busy', 'false');
      expect(button).not.toHaveAttribute('title');
      expect(button).toHaveAttribute('aria-description', label);
      expect(button.textContent).toBe('');
      expect(screen.queryByRole('img', { name: label })).not.toBeInTheDocument();
      expect(screen.queryByText(i18n.t('driver.states.ready'))).not.toBeInTheDocument();
      expect(screen.queryByText(label)).not.toBeInTheDocument();
      view.unmount();
    },
  );

  it.each([
    [true, false, 'audioSession.pause'],
    [false, false, 'audioSession.resume'],
    [true, true, 'audioSession.changing'],
    [false, true, 'audioSession.changing'],
  ] as const)(
    'shows confirmed power %s with transition %s',
    async (enabled, transitioning, key) => {
      await i18n.changeLanguage(locale);
      const view = fixture({ ...healthy, enginePower: { enabled, transitioning } });
      const button = await screen.findByRole('button', { name: i18n.t(key) });
      expect(button).toHaveAttribute('aria-pressed', String(!enabled));
      expect(button).toHaveAttribute('aria-busy', String(transitioning));
      // A transition is announced by aria-busy and its label, not by colour.
      expect(button).toHaveAttribute('data-tone', 'neutral');
      expect(button).toHaveAttribute('aria-description', i18n.t('routing.nodeStatus.running'));
      // The fixture has no power mutation capability, so it must remain read-only.
      expect(button).toBeDisabled();
      expect(button.textContent).toBe('');
      view.unmount();
    },
  );

  it.each([
    ['running', null, 'neutral', 'routing.nodeStatus.running'],
    ['running', 'processing_failed', 'critical', 'routing.nodeStatus.failed'],
    ['idle', null, 'neutral', 'routing.nodeStatus.idle'],
    // Only a failed session colours the shared pause/resume control.
    [undefined, null, 'neutral', 'routing.nodeStatus.unknown'],
    ['stopped', null, 'critical', 'routing.nodeStatus.stopped'],
  ] as const)(
    'keeps %s/%s health independent from confirmed power',
    async (processingState, issue, tone, key) => {
      await i18n.changeLanguage(locale);
      const view = fixture({
        ...healthy,
        audioSession: { ...session, processingState, issue },
        enginePower: { enabled: true, transitioning: false },
      });
      const action = i18n.t('audioSession.pause');
      const health = i18n.t(key);
      const button = await screen.findByRole('button', { name: action });
      expect(button).toHaveAttribute('aria-pressed', 'false');
      expect(button).toHaveAttribute('data-tone', tone);
      expect(button).toHaveAttribute('aria-description', health);
      expect(button).not.toHaveAttribute('title');
      view.unmount();
    },
  );
});
