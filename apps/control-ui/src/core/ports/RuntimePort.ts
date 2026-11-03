import type { RuntimeConnectStage, RuntimeStatus } from '../domain/runtime';

/** Audio belongs to MixRelay Driver; the editor explicitly connects before loading a workspace. */
export interface RuntimePort {
  /** Reports each step as the native side reaches it, then resolves with the outcome. */
  connect(onStage: (stage: RuntimeConnectStage) => void): Promise<RuntimeStatus>;
  /** Read-only: observing a stopped runtime must never restart audio. */
  status(): Promise<RuntimeStatus>;
}
