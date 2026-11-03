import { ControlPortError } from '../../../core/domain/control/errors';
import type { RecorderStatus } from '../../../core/domain/control/recorder';

export function recorderError(error: unknown): string {
  if (error instanceof ControlPortError) {
    const parameters = error.body.parameters;
    if (
      typeof parameters === 'object' &&
      parameters !== null &&
      'reason' in parameters &&
      typeof parameters.reason === 'string'
    )
      return parameters.reason;
  }
  return error instanceof Error ? error.message : String(error);
}

export function recordingDuration(status: RecorderStatus | null): string {
  const milliseconds = status?.sampleRate
    ? Math.floor((status.frames * 1000) / status.sampleRate)
    : 0;
  const seconds = Math.floor(milliseconds / 1000);
  const clock = [Math.floor(seconds / 3600), Math.floor(seconds / 60) % 60, seconds % 60]
    .map((part) => String(part).padStart(2, '0'))
    .join(':');
  return `${clock}.${String(milliseconds % 1000).padStart(3, '0')}`;
}
