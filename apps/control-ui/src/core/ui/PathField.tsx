import { Input, Label, TextField } from 'react-aria-components';
import { IconActionButton } from './IconActionButton';
import styles from './PathField.module.css';

/** A read-only path stays selectable; choosing a destination is a separate action. */
export function PathField({
  label,
  value,
  actionLabel,
  disabled,
  onChoose,
}: {
  label: string;
  value: string;
  actionLabel: string;
  disabled: boolean;
  onChoose?: () => void;
}) {
  return (
    <TextField className={styles.root} isReadOnly>
      <Label className={styles.label}>{label}</Label>
      <div className={styles.wrapper}>
        <Input className={styles.path} value={value} />
        {onChoose ? (
          <IconActionButton
            label={actionLabel}
            disabled={disabled}
            density="regular"
            onClick={onChoose}
          >
            <svg aria-hidden="true" viewBox="0 0 20 20">
              <path d="M2 6V4h6l2 2h8v3M2 6v11h14l2-8H5l-3 8" />
            </svg>
          </IconActionButton>
        ) : null}
      </div>
    </TextField>
  );
}
