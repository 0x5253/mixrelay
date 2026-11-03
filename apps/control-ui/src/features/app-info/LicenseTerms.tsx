import { useTranslation } from 'react-i18next';
import sourceLicense from '../../../../../LICENSE.txt?raw';
import plexLicense from '../../core/theme/fonts/OFL.txt?raw';
import styles from './LegalNotices.module.css';

/** Product and third-party license disclosures. */
export function LicenseTerms() {
  const { t } = useTranslation();
  return (
    <div className={styles.terms}>
      <details>
        <summary>{t('about.termsTitle')}</summary>
        <pre className={styles.termsText} lang="en">
          {sourceLicense}
        </pre>
      </details>
      <details>
        <summary>IBM Plex · SIL Open Font License 1.1</summary>
        <pre className={styles.termsText} lang="en">
          {plexLicense}
        </pre>
      </details>
    </div>
  );
}
