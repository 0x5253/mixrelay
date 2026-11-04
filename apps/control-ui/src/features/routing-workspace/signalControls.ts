import type { GraphNodeControls, NodeControlState } from '../../core/domain/control/graph';

/** Which of Off and Mute the engine can switch for a node. */
export type SignalControlCapabilities = Pick<
  GraphNodeControls['nodes'][number]['capabilities'],
  'power' | 'mute'
>;

/** Which of Off and Mute a node shows. */
export interface SignalControlVisibility {
  power: boolean;
  mute: boolean;
}

const RECOVERY_ONLY_TYPES: ReadonlySet<string> = new Set(['effect', 'compressor', 'limiter']);

/**
 * Built-in processors are switched with their own Bypass, so outside MIDI mapping they show
 * Off and Mute only to recover a state authored elsewhere.
 */
export function usesRecoveryControls(runtimeType: string | undefined, mapping: boolean): boolean {
  return !mapping && runtimeType !== undefined && RECOVERY_ONLY_TYPES.has(runtimeType);
}

/**
 * The controls a node shows, or `null` when a recovery-only node has nothing to recover.
 * A control appears only where the engine reports the capability.
 */
export function signalControlVisibility(
  capabilities: SignalControlCapabilities | undefined,
  control: NodeControlState,
  recoveryOnly: boolean,
): SignalControlVisibility | null {
  if (recoveryOnly && control.enabled && !control.muted) return null;
  return {
    power: capabilities?.power === true && (!recoveryOnly || !control.enabled),
    mute: capabilities?.mute === true && (!recoveryOnly || control.muted),
  };
}
