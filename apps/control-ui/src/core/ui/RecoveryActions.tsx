import { ActionButton } from './ActionButton';
import { FormActions } from './FormActions';

export function RecoveryActions({
  retryLabel,
  reloadLabel,
  onRetry,
  onReload,
}: {
  retryLabel: string;
  reloadLabel: string;
  onRetry: () => void;
  onReload: () => void;
}) {
  return (
    <FormActions align="start">
      <ActionButton onClick={onRetry}>{retryLabel}</ActionButton>
      <ActionButton onClick={onReload}>{reloadLabel}</ActionButton>
    </FormActions>
  );
}
