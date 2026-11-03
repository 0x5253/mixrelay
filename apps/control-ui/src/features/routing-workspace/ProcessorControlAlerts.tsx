import { useTranslation } from 'react-i18next';

import { ActionButton } from '../../core/ui/ActionButton';
import { InlineNotice } from '../../core/ui/InlineNotice';
import type { ProcessorEditorIssue } from './application/EditorSession';

export function ProcessorEditorAlert({
  issue,
  onRetry,
}: {
  issue: ProcessorEditorIssue;
  onRetry: () => void;
}) {
  const { t } = useTranslation();
  return (
    <InlineNotice
      tone="warning"
      announcement="alert"
      actions={
        <ActionButton disabled={issue.busy} onClick={onRetry}>
          {t('routing.processorControl.retry')}
        </ActionButton>
      }
    >
      {t('routing.processorControl.editor', { name: issue.name })}
    </InlineNotice>
  );
}
