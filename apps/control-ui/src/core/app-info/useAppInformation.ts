import { useContext, useEffect, useRef, useState } from 'react';
import { version as buildVersion } from '../../../package.json';
import { removableSubscription } from '../application/subscription';
import { NativeShellContext } from '../shell/NativeShellContext';

/** The native menu opens product information. */
export function useAppInformation(onOpen: () => void, enabled: boolean) {
  const shell = useContext(NativeShellContext);
  const [version, setVersion] = useState(buildVersion);
  const open = useRef(onOpen);
  open.current = onOpen;
  useEffect(() => {
    if (!enabled || !shell) return;
    let disposed = false;
    void shell
      .version()
      .then((value) => {
        if (!disposed) setVersion(value);
      })
      .catch((error: unknown) => console.error('Could not read app version', error));
    const stop = removableSubscription(
      shell.onAbout(() => open.current()),
      (error) => console.error('About menu listener failed', error),
    );
    return () => {
      disposed = true;
      stop();
    };
  }, [enabled, shell]);
  return version;
}
