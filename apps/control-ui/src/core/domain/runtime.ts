export type RuntimeStatus = 'ready' | 'not_installed' | 'incompatible' | 'unavailable';

/** Steps of an explicit connection, in order. A Driver that is already running skips `starting`. */
export const RUNTIME_CONNECT_STAGES = ['checking', 'starting', 'preparing_engine'] as const;
export type RuntimeConnectStage = (typeof RUNTIME_CONNECT_STAGES)[number];

export function isRuntimeConnectStage(value: unknown): value is RuntimeConnectStage {
  return RUNTIME_CONNECT_STAGES.some((stage) => stage === value);
}
