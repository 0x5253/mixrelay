import { ControlPortError, type ProtocolErrorBody } from '../../domain/control/errors';
import { type Decoder, InvalidResponseError } from '../decode';
import { protocolErrorBody } from './protocolDecoders';

/** A reply that does not match its contract. The request may still have been applied, so
 * it is reported as a server-side failure, never as a definite rejection. */
export const INVALID_RESPONSE_STATUS = 502;

/** The typed reply, or a ControlPortError naming where it broke the contract. */
export function decodeReply<T>(decoder: Decoder<T>, body: unknown, name: string): T {
  try {
    return decoder(body, name);
  } catch (error) {
    if (!(error instanceof InvalidResponseError)) throw error;
    throw new ControlPortError(INVALID_RESPONSE_STATUS, {
      code: 'control_request_failed',
      parameters: { reason: 'invalid_response', path: error.path },
    });
  }
}

/** A refusal keeps its status; an unreadable error body still reports that status. */
export function refusal(status: number, body: unknown): ControlPortError {
  let decoded: ProtocolErrorBody;
  try {
    decoded = protocolErrorBody(body, 'error');
  } catch {
    decoded = { code: 'control_request_failed', parameters: { reason: 'invalid_error_body' } };
  }
  return new ControlPortError(status, decoded);
}
