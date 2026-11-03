import { beforeEach, expect, it } from 'vitest';
import {
  getAudioSessionPresentation,
  getEnginePresentation,
} from '../../core/audio-session/presentation';
import type {
  AudioSessionStatus,
  DriverState,
  MixRelayStatus,
} from '../../core/domain/control/status';
import { i18n } from '../../core/i18n';

function session(overrides: Partial<AudioSessionStatus> = {}): AudioSessionStatus {
  return {
    format: { sampleRate: 48000, blockFrames: 128 },
    clockAuthority: 'engine_timer',
    observedDeviceSampleRate: null,
    issue: null,
    transportLatencyFrames: null,
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
    ...overrides,
  };
}

function status(driverState: DriverState, audioSession: AudioSessionStatus): MixRelayStatus {
  return {
    protocolVersion: 1,
    driverState,
    sampleRate: 48000,
    bufferFrames: 128,
    underruns: 0,
    overruns: 0,
    audioSession,
  };
}

beforeEach(async () => {
  await i18n.changeLanguage('en');
});

it('reports an unknown owner until the actual session provides processing-state evidence', () => {
  expect(getAudioSessionPresentation(undefined, i18n.t)).toEqual({
    label: 'Audio status unknown',
    tone: 'warning',
  });
  expect(getEnginePresentation(null, i18n.t)).toEqual({
    label: 'Audio status unknown',
    tone: 'warning',
  });
});

it.each(['running', 'idle'] as const)(
  'presents confirmed %s processing independently of the driver installation',
  (processingState) => {
    const result = getAudioSessionPresentation(session({ processingState }), i18n.t);
    expect(result.label).toBe(i18n.t(`routing.nodeStatus.${processingState}`));
    // Idle is ready, not degraded: it must not look like a waiting or failing session.
    expect(result.tone).toBe('positive');
  },
);

it('keeps stopped or failed processing critical even when a different driver issue is present', () => {
  expect(
    getEnginePresentation(status('not_installed', session({ processingState: 'stopped' })), i18n.t),
  ).toEqual({ label: i18n.t('routing.nodeStatus.stopped'), tone: 'critical' });
  expect(
    getEnginePresentation(
      status('blocked', session({ processingState: 'running', issue: 'processing_failed' })),
      i18n.t,
    ),
  ).toEqual({ label: i18n.t('routing.nodeStatus.failed'), tone: 'critical' });
});

it('shows runtime format and unsaved preference issues without claiming every device has failed', () => {
  expect(
    getAudioSessionPresentation(
      session({ processingState: 'running', issue: 'clock_not_synchronized' }),
      i18n.t,
    ),
  ).toEqual({ label: i18n.t('routing.nodeStatus.issues.clock_not_synchronized'), tone: 'warning' });
  expect(
    getAudioSessionPresentation(
      session({ processingState: 'running', endpointPreferencesUnavailable: true }),
      i18n.t,
    ),
  ).toEqual({
    label: i18n.t('routing.nodeStatus.issues.preferences_unavailable'),
    tone: 'warning',
  });
});

it.each([
  { state: 'not_installed' as const, label: 'notInstalled', tone: 'warning' },
  { state: 'blocked' as const, label: 'blocked', tone: 'critical' },
  { state: 'version_mismatch' as const, label: 'versionMismatch', tone: 'warning' },
])(
  'uses the driver $state warning when otherwise healthy audio cannot connect',
  ({ state, label, tone }) => {
    expect(
      getEnginePresentation(status(state, session({ processingState: 'running' })), i18n.t),
    ).toEqual({ label: i18n.t(`driver.states.${label}`), tone });
  },
);
