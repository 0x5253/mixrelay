import { Button } from 'react-aria-components';
import styles from './AppInformationButton.module.css';
import { Tooltip } from './Tooltip';

/** A version action linked to the product information dialog. */
export function AppInformationButton({
  label,
  version,
  opened,
  onOpen,
}: {
  label: string;
  version: string;
  opened: boolean;
  onOpen: () => void;
}) {
  const summary = version;
  return (
    <Tooltip label={`${label} · ${summary}`}>
      <Button
        className={styles.root}
        render={(props) => <button {...props} aria-description={summary} />}
        aria-label={label}
        aria-description={summary}
        aria-expanded={opened}
        aria-haspopup="dialog"
        onClick={onOpen}
      >
        <span className={styles.summary}>
          <span className={styles.version}>{version}</span>
        </span>
      </Button>
    </Tooltip>
  );
}
