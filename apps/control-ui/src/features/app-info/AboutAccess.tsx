import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useAppInformation } from '../../core/app-info/useAppInformation';
import { AppInformationButton } from '../../core/ui/AppInformationButton';
import { AboutDialog } from './AboutDialog';

export function AboutAccess({ children }: { children: (action: ReactNode) => ReactNode }) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState(false);
  const version = useAppInformation(() => setOpened(true), true);
  return (
    <>
      {children(
        <AppInformationButton
          label={t('about.title')}
          version={version}
          opened={opened}
          onOpen={() => setOpened(true)}
        />,
      )}
      <AboutDialog version={version} opened={opened} onClose={() => setOpened(false)} />
    </>
  );
}
