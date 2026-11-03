import { useTranslation } from 'react-i18next';
import { Dialog } from '../../core/ui/Dialog';
import { Stack } from '../../core/ui/Stack';
import { Text } from '../../core/ui/Text';
import { LicenseTerms } from './LicenseTerms';

/** Product identity and legal information. */
export function AboutDialog({
  version,
  opened,
  onClose,
}: {
  version: string;
  opened: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Dialog
      title={t('about.title')}
      closeLabel={t('common.close')}
      opened={opened}
      onClose={onClose}
    >
      <Stack>
        <Text kind="supporting" tone="muted">
          {t('about.version', { version })}
        </Text>
        <Text>rawsector</Text>
        <Text>
          <a href="mailto:support@rawsector.com">support@rawsector.com</a>
        </Text>
        <LicenseTerms />
      </Stack>
    </Dialog>
  );
}
