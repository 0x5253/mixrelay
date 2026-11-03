import { Button, Menu, MenuItem, MenuTrigger, Popover } from 'react-aria-components';
import { useTranslation } from 'react-i18next';
import { themePreference } from '../domain/appearance';
import styles from '../theme/ComponentTheme.module.css';
import { useTheme } from '../theme/MixRelayProvider';
import iconStyles from './IconActionButton.module.css';
import pickerStyles from './ThemePicker.module.css';
import { Tooltip } from './Tooltip';

/** One focus owner, checked menu choices, and React Aria keyboard/focus behavior. */
export function ThemePicker() {
  const { t } = useTranslation();
  const { preference, setPreference } = useTheme();
  const label = t('theme.current', { theme: t(`theme.${preference}`) });
  return (
    <MenuTrigger>
      <Tooltip label={label}>
        <Button className={iconStyles.root} aria-label={label}>
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <circle cx="8" cy="8" r="5" />
            <path d="M8 3v10M8 3a5 5 0 0 1 0 10Z" fill="currentColor" />
          </svg>
        </Button>
      </Tooltip>
      <Popover
        className={`${styles.selectPopover} ${pickerStyles.popover}`}
        placement="bottom end"
        offset={4}
      >
        <Menu
          className={styles.listBox}
          aria-label={t('theme.label')}
          selectionMode="single"
          selectedKeys={[preference]}
          onAction={(key) => setPreference(themePreference(key))}
        >
          {(['system', 'light', 'dark'] as const).map((value) => (
            <MenuItem
              id={value}
              key={value}
              textValue={t(`theme.${value}`)}
              className={styles.option}
            >
              <span>{t(`theme.${value}`)}</span>
              <span aria-hidden="true">{preference === value ? '✓' : ''}</span>
            </MenuItem>
          ))}
        </Menu>
      </Popover>
    </MenuTrigger>
  );
}
