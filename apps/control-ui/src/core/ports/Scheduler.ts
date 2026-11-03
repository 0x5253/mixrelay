/** Cancels a scheduled task; calling it after the task ran or twice does nothing. */
export type Cancel = () => void;

/** Runs a task later. Adapters and application coordinators that wait take it, so tests drive
 * time; view code may default to the browser timers. */
export interface Scheduler {
  after(delayMs: number, task: () => void): Cancel;
}

/** Wall-clock milliseconds, as compared between successive readings. */
export interface Clock {
  now(): number;
}

/** Elapsed milliseconds from a stable origin, independent of wall-clock corrections. */
export interface MonotonicClock {
  now(): number;
}
