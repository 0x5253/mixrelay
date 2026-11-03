import { ActionButton } from './ActionButton';
import { InlineNotice } from './InlineNotice';

/** A recoverable operation failure; retry never discards the caller's last valid data. */
export function RetryAlert({
  message,
  retryLabel,
  busy,
  onRetry,
}: {
  message: string;
  retryLabel: string;
  busy: boolean;
  onRetry: () => void;
}) {
  return (
    <InlineNotice
      tone="critical"
      actions={
        <ActionButton disabled={busy} onClick={onRetry}>
          {retryLabel}
        </ActionButton>
      }
    >
      {message}
    </InlineNotice>
  );
}
