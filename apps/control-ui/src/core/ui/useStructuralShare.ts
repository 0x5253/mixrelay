import { type SetStateAction, useCallback, useRef, useState } from 'react';
import { structuralShare } from '../domain/structuralShare';

/** `structuralShare` against the previous render's value. */
export function useStructuralShare<T>(value: T): T {
  const shared = useRef(value);
  shared.current = structuralShare(shared.current, value);
  return shared.current;
}

/** `useState` whose updates keep the previous value when the next one is deep-equal, so a
 * poll that observes nothing new does not re-render its consumers. */
export function useSharedState<T>(initial: T | (() => T)) {
  const [value, setValue] = useState(initial);
  const set = useCallback(
    (next: SetStateAction<T>) =>
      setValue((previous) =>
        structuralShare(
          previous,
          typeof next === 'function' ? (next as (previous: T) => T)(previous) : next,
        ),
      ),
    [],
  );
  return [value, set] as const;
}
