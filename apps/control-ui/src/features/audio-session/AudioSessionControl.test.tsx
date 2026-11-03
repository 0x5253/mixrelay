import { render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { AudioSessionStatus, MixRelayStatus } from '../../core/domain/control/status';
import { i18n } from '../../core/i18n';
import { en } from '../../core/i18n/locales/en';
import { koKR } from '../../core/i18n/locales/ko-KR';
import { pseudo } from '../../core/i18n/locales/pseudo';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioSessionControl } from './AudioSessionControl';

const hiddenDiagnostics = [
  'backend',
  'deviceRate',
  'processingRate',
  'clock',
  'conversion',
  'drift',
  'syncNotice',
  'changeNotice',
];

it('retains recorder finalization errors even without an audio session or recorder node', async () => {
  await i18n.changeLanguage('en');
  const status: MixRelayStatus = {
    protocolVersion: 1,
    driverState: 'ready',
    sampleRate: null,
    bufferFrames: null,
    underruns: 0,
    overruns: 0,
    recorderError: 'take.wav: Disk full',
  };
  const controlPort = new StorybookMockControlPort({
    status,
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
  });
  render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <AudioSessionControl
          status={status}
          controlPort={controlPort}
          onApplied={vi.fn()}
          onBusyChange={vi.fn()}
        />
      </I18nextProvider>
    </MixRelayProvider>,
  );
  expect(screen.getByRole('alert')).toHaveTextContent('Recording error: take.wav: Disk full');
});

it('keeps the same buffer control and value when playback pauses or configuration locks', async () => {
  await i18n.changeLanguage('en');
  const session: AudioSessionStatus = {
    format: { sampleRate: 48000, blockFrames: 256 },
    clockAuthority: 'engine_timer',
    observedDeviceSampleRate: null,
    issue: null,
    transportLatencyFrames: null,
    capabilities: {
      backend: 'core_audio',
      supportedSampleRates: [48000],
      supportedBlockFrames: [128, 256],
      canReconfigureLive: true,
      hardwareInput: true,
      hardwareOutput: true,
      virtualEndpoint: true,
      applicationCapture: true,
      sampleRateConversion: true,
      driftCorrection: true,
    },
  };
  const status: MixRelayStatus = {
    protocolVersion: 1,
    driverState: 'ready',
    sampleRate: 48000,
    bufferFrames: 256,
    underruns: 0,
    overruns: 0,
    audioSession: session,
    enginePower: { enabled: true, transitioning: false },
  };
  const port = new StorybookMockControlPort({
    status,
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
  });
  const view = (next: MixRelayStatus) => (
    <MixRelayProvider>
      <AudioSessionControl
        status={next}
        controlPort={port}
        onApplied={vi.fn()}
        onBusyChange={vi.fn()}
      />
    </MixRelayProvider>
  );
  const rendered = render(view(status));
  const buffer = screen.getByRole('button', {
    name: (name) => name.endsWith(i18n.t('audioSession.processingBuffer')),
  });
  expect(buffer).toHaveTextContent('256');
  rendered.rerender(view({ ...status, enginePower: { enabled: false, transitioning: false } }));
  expect(
    screen.getByRole('button', {
      name: (name) => name.endsWith(i18n.t('audioSession.processingBuffer')),
    }),
  ).toBe(buffer);
  expect(buffer).toHaveTextContent('256');
  rendered.rerender(
    view({
      ...status,
      audioSession: {
        ...session,
        capabilities: { ...session.capabilities, canReconfigureLive: false },
      },
    }),
  );
  expect(
    screen.getByRole('button', {
      name: (name) => name.endsWith(i18n.t('audioSession.processingBuffer')),
    }),
  ).toBe(buffer);
  expect(buffer).toBeDisabled();
  expect(buffer).toHaveTextContent('256');
  expect(buffer.closest('[title]')).toHaveAttribute('title', i18n.t('audioSession.locked'));
});

it.each(
  ['en', 'ko-KR', 'pseudo'].flatMap((language) =>
    [null, { sampleRate: 48000, blockFrames: 256 }, { sampleRate: 44100, blockFrames: 128 }].map(
      (preferredFormat) => ({ language, preferredFormat }),
    ),
  ),
)(
  'does not expose separate active/saved settings: $language $preferredFormat',
  async ({ language, preferredFormat }) => {
    await i18n.changeLanguage(language);
    const status: MixRelayStatus = {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 256,
      underruns: 0,
      overruns: 0,
      audioSession: {
        format: { sampleRate: 48000, blockFrames: 256 },
        preferredFormat,
        clockAuthority: 'engine_timer',
        observedDeviceSampleRate: 48000,
        issue: 'stored_format_unavailable',
        transportLatencyFrames: null,
        capabilities: {
          backend: 'core_audio',
          supportedSampleRates: [44100, 48000],
          supportedBlockFrames: [128, 256],
          canReconfigureLive: true,
          hardwareInput: true,
          hardwareOutput: true,
          virtualEndpoint: true,
          applicationCapture: true,
          sampleRateConversion: false,
          driftCorrection: false,
        },
      },
    };
    const controlPort = new StorybookMockControlPort({
      status,
      graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
      graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    });
    render(
      <I18nextProvider i18n={i18n}>
        <MixRelayProvider>
          <AudioSessionControl
            status={status}
            controlPort={controlPort}
            onApplied={vi.fn()}
            onBusyChange={vi.fn()}
          />
        </MixRelayProvider>
      </I18nextProvider>,
    );
    expect(screen.queryByText(i18n.t('audioSession.diagnostics'))).not.toBeInTheDocument();
    expect(screen.getByText(`${i18n.t('audioSession.bufferLatency')} 5.33 ms`)).toBeInTheDocument();
    // Keep one editable settings form; native and persisted values remain an internal contract.
    expect(screen.queryByRole('button', { name: i18n.t('audioSession.sampleRate') })).toBeNull();
    expect(
      screen.getAllByRole('button', {
        name: (name) => name.endsWith(i18n.t('audioSession.processingBuffer')),
      }),
    ).toHaveLength(1);
    expect(
      screen.getByRole('button', {
        name: (name) => name.endsWith(i18n.t('audioSession.processingBuffer')),
      }),
    ).toHaveTextContent('256');
    expect(screen.getAllByText(i18n.t('audioSession.processingBuffer'))).toHaveLength(1);
    for (const key of hiddenDiagnostics)
      expect(screen.queryByText(i18n.t(`audioSession.${key}`))).not.toBeInTheDocument();
    for (const label of [
      'Active format',
      'Saved format',
      'Not saved',
      '현재 적용값',
      '저장된 설정',
      '저장되지 않음',
      '[Active format]',
      '[Saved format]',
      '[Not saved]',
      'Block size',
      'Device rate',
      '블록 크기',
      '장치 레이트',
    ])
      expect(screen.queryByText(label)).not.toBeInTheDocument();
    // Simplification must not hide a failed application or change the stored/runtime values.
    expect(
      screen.getByText(i18n.t('audioSession.issues.stored_format_unavailable')),
    ).toBeInTheDocument();
    expect(status.audioSession?.preferredFormat).toEqual(preferredFormat);
    expect(status.audioSession?.format).toEqual({ sampleRate: 48000, blockFrames: 256 });
  },
);

it.each(
  ['en', 'ko-KR', 'pseudo'].flatMap((language) =>
    ([null, 'clock_not_synchronized', 'block_size_mismatch'] as const).map((issue) => ({
      language,
      issue,
    })),
  ),
)(
  'shows persistence failure independently of session issue $issue in $language',
  async ({ language, issue }) => {
    await i18n.changeLanguage(language);
    const session: AudioSessionStatus = {
      endpointPreferencesUnavailable: true,
      format: { sampleRate: 48000, blockFrames: 128 },
      clockAuthority: 'engine_timer',
      observedDeviceSampleRate: null,
      observedDeviceBufferFrames: issue === 'block_size_mismatch' ? 512 : null,
      issue,
      transportLatencyFrames: null,
      capabilities: {
        backend: 'contract_only',
        supportedSampleRates: [48000],
        supportedBlockFrames: [128],
        canReconfigureLive: false,
        hardwareInput: false,
        hardwareOutput: false,
        virtualEndpoint: true,
        applicationCapture: false,
        sampleRateConversion: false,
        driftCorrection: false,
      },
    };
    const status: MixRelayStatus = {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
      audioSession: session,
      recorderError: 'Could not finalize take.wav',
    };
    const controlPort = new StorybookMockControlPort({
      status,
      graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
      graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    });
    render(
      <I18nextProvider i18n={i18n}>
        <MixRelayProvider>
          <AudioSessionControl
            status={status}
            controlPort={controlPort}
            onApplied={vi.fn()}
            onBusyChange={vi.fn()}
          />
        </MixRelayProvider>
      </I18nextProvider>,
    );
    expect(
      await screen.findByText(i18n.t('audioSession.issues.endpoint_preferences_unavailable')),
    ).toBeInTheDocument();
    if (issue) expect(screen.getByText(i18n.t(`audioSession.issues.${issue}`))).toBeInTheDocument();
    expect(
      screen.getByText(`${i18n.t('recorder.failed')}: Could not finalize take.wav`),
    ).toBeInTheDocument();
    for (const locale of [en, koKR, pseudo])
      expect(locale.audioSession.issues.endpoint_preferences_unavailable).toBeTruthy();
  },
);

it.each([
  { sampleRate: 0, blockFrames: 128 },
  { sampleRate: 48000, blockFrames: 0 },
  { sampleRate: Number.NaN, blockFrames: 128 },
  { sampleRate: 48000, blockFrames: Number.POSITIVE_INFINITY },
])('does not present a fabricated buffer duration for invalid native format %j', async (format) => {
  await i18n.changeLanguage('en');
  const status: MixRelayStatus = {
    protocolVersion: 1,
    driverState: 'ready',
    sampleRate: null,
    bufferFrames: null,
    underruns: 0,
    overruns: 0,
    audioSession: {
      format,
      clockAuthority: 'engine_timer',
      observedDeviceSampleRate: null,
      issue: null,
      transportLatencyFrames: null,
      capabilities: {
        backend: 'core_audio',
        supportedSampleRates: [],
        supportedBlockFrames: [],
        canReconfigureLive: false,
        hardwareInput: false,
        hardwareOutput: false,
        virtualEndpoint: false,
        applicationCapture: false,
        sampleRateConversion: false,
        driftCorrection: false,
      },
    },
  };
  const port = new StorybookMockControlPort({
    status,
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
  });
  render(
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <AudioSessionControl
          status={status}
          controlPort={port}
          onApplied={vi.fn()}
          onBusyChange={vi.fn()}
        />
      </MixRelayProvider>
    </I18nextProvider>,
  );
  expect(screen.getByText(`${i18n.t('audioSession.bufferLatency')} —`)).toBeInTheDocument();
});
