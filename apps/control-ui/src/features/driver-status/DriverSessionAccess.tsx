import type { ReactNode } from 'react';
import type { DriverPort } from '../../core/ports/DriverPort';
import { type DriverSession, useDriverSession } from './useDriverSession';

/** The Driver owns device settings; graph nodes provide their editing UI. */
export function DriverSessionAccess({
  port,
  children,
}: {
  port: DriverPort | null;
  children: (session: DriverSession) => ReactNode;
}) {
  const session = useDriverSession(port);
  return children(session);
}
