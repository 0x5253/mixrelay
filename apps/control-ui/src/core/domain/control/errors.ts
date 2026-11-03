/** The refusal codes this UI acts on. The wire code stays open-ended; see ProtocolErrorBody. */
export const protocolErrorCodes = [
  'node_not_found',
  'node_disabled',
  'recorder_failed',
  'audio_session_unsupported',
  'audio_session_processor_state_unavailable',
  'audio_session_reconfigure_failed',
  'audio_device_disconnected',
  'audio_permission_pending',
  'audio_permission_denied',
  'audio_source_unavailable',
  'control_unavailable',
  'control_request_failed',
  'engine_replaced',
  'invalid_operation_id',
  'graph_binding_unsupported',
  'graph_exchange_busy',
  'graph_invalid',
  'graph_revision_stale',
  'processor_unavailable',
] as const;
export type ProtocolErrorCode = (typeof protocolErrorCodes)[number];

/** `code` is open-ended: the engine and the desktop add codes, and an unknown one is still a
 * refusal. ProtocolErrorCode names the codes this UI interprets. */
export interface ProtocolErrorBody {
  code: string;
  parameters: unknown;
}

export class ControlPortError extends Error {
  public constructor(
    public readonly status: number,
    public readonly body: ProtocolErrorBody,
  ) {
    super(body.code);
    this.name = 'ControlPortError';
  }
}

/** Whether `error` is an engine or desktop refusal with one of `codes`. */
export function hasErrorCode(error: unknown, ...codes: readonly ProtocolErrorCode[]): boolean {
  return error instanceof ControlPortError && codes.some((code) => code === error.body.code);
}
