import { type ReactNode, useState } from 'react';

/** Stands in for the diagnostics window's problem list, where workspace failures render. */
export function ProblemsTarget({
  children,
}: {
  children: (target: HTMLDivElement | null) => ReactNode;
}) {
  const [target, setTarget] = useState<HTMLDivElement | null>(null);
  return (
    <>
      {children(target)}
      <div ref={setTarget} />
    </>
  );
}
