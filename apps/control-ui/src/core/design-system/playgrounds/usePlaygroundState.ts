import { useArgs, useEffect, useRef, useState } from 'storybook/preview-api';

/** Keep input local; publish the latest value after child blur handlers have committed it. */
export function usePlaygroundState<T extends object>(args: T) {
  const [, updateArgs] = useArgs();
  const [local, setLocal] = useState(args);
  const latest = useRef(args);
  const external = useRef(args);
  useEffect(() => {
    const changed = Object.keys({ ...external.current, ...args }).some(
      (key) => external.current[key as keyof T] !== args[key as keyof T],
    );
    if (!changed) return;
    external.current = args;
    latest.current = args;
    setLocal(args);
  }, [args]);
  const set = (patch: Partial<T>) => {
    latest.current = { ...latest.current, ...patch };
    setLocal(latest.current);
  };
  const commit = () => {
    const patch = Object.fromEntries(
      Object.entries(latest.current).filter(([key, value]) => value !== args[key as keyof T]),
    );
    if (Object.keys(patch).length) updateArgs(patch);
  };
  return { local, set, commit };
}
