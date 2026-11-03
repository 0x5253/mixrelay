import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { i18n } from '../../core/i18n';
import { AudioSessionNotices } from './AudioSessionNotices';

it('retains recording-finalization errors after the session disappears and clears them on a clean status', async () => {
  await i18n.changeLanguage('en');
  const view = render(
    <AudioSessionNotices
      status={{
        protocolVersion: 1,
        driverState: 'ready',
        sampleRate: null,
        bufferFrames: null,
        underruns: 0,
        overruns: 0,
        recorderError: 'take.rf64: Disk full',
      }}
    />,
  );
  expect(screen.getByRole('alert')).toHaveTextContent('Recording error: take.rf64: Disk full');
  view.rerender(<AudioSessionNotices status={null} />);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

it('displays native processing and preference errors together without a global device-buffer control', async () => {
  await i18n.changeLanguage('en');
  render(
    <AudioSessionNotices
      status={{
        protocolVersion: 1,
        driverState: 'ready',
        sampleRate: 48000,
        bufferFrames: 128,
        underruns: 0,
        overruns: 0,
        audioSession: {
          format: { sampleRate: 48000, blockFrames: 128 },
          clockAuthority: 'engine_timer',
          observedDeviceSampleRate: null,
          transportLatencyFrames: null,
          issue: 'clock_not_synchronized',
          endpointPreferencesUnavailable: true,
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
      }}
    />,
  );
  const alert = screen.getByRole('alert');
  expect(alert).toHaveTextContent(i18n.t('audioSession.issues.clock_not_synchronized'));
  expect(alert).toHaveTextContent(i18n.t('audioSession.issues.endpoint_preferences_unavailable'));
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});
