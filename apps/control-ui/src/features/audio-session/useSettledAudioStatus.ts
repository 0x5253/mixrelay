import type { MixRelayStatus } from '../../core/domain/control/status';
import { useSustainedProblem } from '../../core/ui/useSustainedProblem';

/** A failed processing owner is critical and shown at once; other issues may be brief. */
function hasTransientProblem(status: MixRelayStatus | null) {
  const session = status?.audioSession;
  return (
    status === null ||
    (session?.issue !== 'processing_failed' &&
      Boolean(session?.issue || session?.endpointPreferencesUnavailable))
  );
}

/** One missed poll or a sub-second session issue is not shown: until it has lasted the
 * feedback delay the last good observation stands in for a missing reply, and a live
 * reply is shown without the unconfirmed issue. Lasting problems surface unchanged. */
export function useSettledAudioStatus(status: MixRelayStatus | null) {
  const settled = useSustainedProblem(status, hasTransientProblem);
  if (settled === status || !status) return settled;
  // A live reply is held back only for a session issue, so the session is present.
  const session = status.audioSession as NonNullable<MixRelayStatus['audioSession']>;
  return {
    ...status,
    audioSession: { ...session, issue: null, endpointPreferencesUnavailable: false },
  };
}
