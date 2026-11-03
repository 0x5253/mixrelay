import { useEffect, useLayoutEffect, useReducer, useRef } from 'react';
import { feedback } from '../theme/tokens';

/** Keeps presenting the last healthy value until a problem has lasted `delayMs`; recovery
 * is shown at once. With no healthy value seen yet the problem is shown immediately.
 * History is read during render but recorded only on commit, so an interrupted render
 * never counts as a healthy observation or restarts the wait. */
export function useSustainedProblem<T>(
  value: T,
  isProblem: (value: T) => boolean,
  delayMs: number = feedback.problemDelay,
): T {
  const problem = isProblem(value);
  const healthy = useRef<{ value: T } | null>(null);
  const since = useRef<number | null>(null);
  const [, rerender] = useReducer((count: number) => count + 1, 0);
  const start = problem ? (since.current ?? Date.now()) : null;
  const elapsed = start === null ? 0 : Date.now() - start;
  const waiting = problem && healthy.current !== null && elapsed < delayMs;
  useLayoutEffect(() => {
    if (start === null) {
      healthy.current = { value };
      since.current = null;
    } else since.current = start;
  });
  useEffect(() => {
    if (!waiting) return;
    const timer = setTimeout(rerender, delayMs - elapsed);
    return () => clearTimeout(timer);
  });
  return waiting ? (healthy.current as { value: T }).value : value;
}
