import type {
  RecorderEncoding,
  RecorderSettings,
  RecorderStatus,
} from '../../../core/domain/control/recorder';

/** What the recorder node shows and allows, from its native status and the graph. */
export interface RecorderControlState {
  /** A recording exists (running or paused); its format cannot change. */
  locked: boolean;
  /** No command can be sent: a request is in flight, status is unknown, or the node is off. */
  disabled: boolean;
  /** Folder, channel, rate and encoding fields cannot change. */
  fieldsLocked: boolean;
  record: { enabled: boolean; resumes: boolean; active: boolean };
  pause: { enabled: boolean };
  stop: { enabled: boolean };
  /** The running recording's own values while locked, otherwise the saved settings. */
  sampleRate: number | null;
  encoding: RecorderEncoding;
}

export interface RecorderControlInputs {
  status: RecorderStatus | null;
  settings: RecorderSettings | null;
  settingsError: string | null;
  busy: boolean;
  /** The graph routes audio into this recorder. */
  enabled: boolean;
  poweredOn: boolean;
  /** App playback is paused; a paused source cannot start or keep feeding a take. */
  playbackPaused: boolean;
}

export function recorderControlState(inputs: RecorderControlInputs): RecorderControlState {
  const { status, settings } = inputs;
  const locked = status?.state === 'recording' || status?.state === 'paused';
  const disabled = inputs.busy || !status || !inputs.poweredOn;
  const canStart = inputs.enabled && !!settings && !inputs.settingsError;
  return {
    locked,
    disabled,
    fieldsLocked: disabled || locked || !inputs.enabled || !settings,
    record: {
      enabled:
        !disabled &&
        !inputs.playbackPaused &&
        status?.state !== 'recording' &&
        (locked || canStart),
      resumes: status?.state === 'paused',
      active: status?.state === 'recording',
    },
    pause: { enabled: !disabled && !inputs.playbackPaused && status?.state === 'recording' },
    stop: { enabled: !disabled && locked },
    sampleRate: (locked ? status?.sampleRate : settings?.sampleRate) ?? null,
    encoding: (locked ? status?.encoding : settings?.encoding) ?? 'pcm16',
  };
}

const sampleRates = [44100, 48000, 88200, 96000, 176400, 192000];

/** The product rates, plus a rate the running recording uses that is not among them. */
export function recordingRateOptions(observed: number | null): { value: string; label: string }[] {
  return [...new Set([...sampleRates, ...(observed ? [observed] : [])])].map((rate) => ({
    value: String(rate),
    label: `${rate / 1000} kHz`,
  }));
}
