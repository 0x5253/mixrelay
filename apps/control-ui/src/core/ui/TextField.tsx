import { type ComponentPropsWithoutRef, type ReactNode, useState } from 'react';
import { TextField as AriaTextField, Button, Input, Label, Text } from 'react-aria-components';
import { useTranslation } from 'react-i18next';
import styles from '../theme/ComponentTheme.module.css';
import type { NoAppearanceOverrides } from './ActionButton';
import { Tooltip } from './Tooltip';

type TextFieldProps = Pick<
  ComponentPropsWithoutRef<'input'>,
  | 'value'
  | 'defaultValue'
  | 'placeholder'
  | 'disabled'
  | 'readOnly'
  | 'required'
  | 'maxLength'
  | 'minLength'
  | 'name'
  | 'id'
  | 'type'
  | 'autoComplete'
  | 'autoFocus'
  | 'onChange'
  | 'onBlur'
  | 'onFocus'
  | 'onKeyDown'
  | 'aria-label'
  | 'aria-describedby'
> &
  NoAppearanceOverrides & {
    label?: string;
    description?: ReactNode;
    error?: ReactNode;
    'data-autofocus'?: boolean;
  };

export function TextField({
  label,
  description,
  error,
  disabled,
  readOnly,
  required,
  defaultValue,
  ...props
}: TextFieldProps) {
  const { t } = useTranslation();
  const [revealed, setRevealed] = useState(false);
  const password = props.type === 'password';
  const passwordActionLabel = [
    props['aria-label'] ?? label,
    t(revealed ? 'common.hidePassword' : 'common.showPassword'),
  ]
    .filter(Boolean)
    .join(': ');
  const input = (
    <Input
      {...props}
      type={password && revealed ? 'text' : props.type}
      className={styles.input}
      aria-invalid={Boolean(error) || undefined}
      autoFocus={props.autoFocus || props['data-autofocus']}
    />
  );
  return (
    <AriaTextField
      className={styles.field}
      isDisabled={disabled}
      isReadOnly={readOnly}
      isRequired={required}
      isInvalid={Boolean(error)}
      defaultValue={defaultValue === undefined ? undefined : String(defaultValue)}
      aria-label={props['aria-label']}
    >
      {label ? <Label className={styles.fieldLabel}>{label}</Label> : null}
      {password ? (
        <div className={styles.passwordField}>
          {input}
          <Tooltip label={passwordActionLabel}>
            <Button
              className={styles.passwordToggle}
              isDisabled={disabled}
              aria-label={passwordActionLabel}
              aria-pressed={revealed}
              onPress={() => setRevealed((value) => !value)}
            >
              <svg
                aria-hidden="true"
                viewBox="0 0 16 16"
                width="16"
                height="16"
                fill="none"
                stroke="currentColor"
              >
                <path d="M1 8s2.5-4 7-4 7 4 7 4-2.5 4-7 4-7-4-7-4Z" />
                <circle cx="8" cy="8" r="2" />
                {revealed ? <path d="m2 2 12 12" /> : null}
              </svg>
            </Button>
          </Tooltip>
        </div>
      ) : (
        input
      )}
      {description ? (
        <Text slot="description" className={styles.fieldDescription}>
          {description}
        </Text>
      ) : null}
      {error ? (
        <Text slot="errorMessage" role="alert" className={styles.fieldError}>
          {error}
        </Text>
      ) : null}
    </AriaTextField>
  );
}
