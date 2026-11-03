import { Switch } from 'react-aria-components';
import styles from '../theme/ComponentTheme.module.css';

export function ToggleField({
  checked,
  label,
  onChange,
  disabled,
}: {
  checked: boolean;
  label: string;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <Switch
      className={styles.toggle}
      isSelected={checked}
      isDisabled={disabled}
      onChange={onChange}
    >
      <span aria-hidden="true" className={styles.toggleTrack}>
        <span className={styles.toggleThumb} />
      </span>
      <span>{label}</span>
    </Switch>
  );
}
