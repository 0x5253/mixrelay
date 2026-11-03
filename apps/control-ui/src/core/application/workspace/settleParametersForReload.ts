import { ControlPortError } from '../../domain/control/errors';

export async function settleParametersForReload(whenIdle: () => Promise<void>) {
  try {
    await whenIdle();
  } catch (error: unknown) {
    // Settled parameter failures remain visible until the explicit reload succeeds.
    if (!(error instanceof ControlPortError)) throw error;
  }
}
