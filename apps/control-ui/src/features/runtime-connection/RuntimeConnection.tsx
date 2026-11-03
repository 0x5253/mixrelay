import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  RUNTIME_CONNECT_STAGES,
  type RuntimeConnectStage,
  type RuntimeStatus,
} from '../../core/domain/runtime';
import type { RuntimePort } from '../../core/ports/RuntimePort';
import { ActionButton } from '../../core/ui/ActionButton';
import { BrandIdentity } from '../../core/ui/BrandIdentity';
import { Inline } from '../../core/ui/Inline';
import { ProgressBar } from '../../core/ui/ProgressBar';
import { Stack } from '../../core/ui/Stack';
import { Text } from '../../core/ui/Text';
import styles from './RuntimeConnection.module.css';

import { useRuntimeConnection } from './useRuntimeConnection';

/** Mount the workspace only after Driver and engine readiness; preserve it during disconnection. */
export function RuntimeConnection({
  port,
  onInstall,
  children,
}: {
  port: RuntimePort;
  onInstall?: () => void;
  children: ReactNode;
}) {
  const { state, stage, opened, connect } = useRuntimeConnection(port);

  return (
    <>
      {opened ? (
        <div hidden={state !== 'ready'} inert={state !== 'ready'}>
          {children}
        </div>
      ) : null}
      {state !== 'ready' ? (
        <ConnectionScreen state={state} stage={stage} onInstall={onInstall} onRetry={connect} />
      ) : null}
    </>
  );
}

function ConnectingScreen({ stage }: { stage: RuntimeConnectStage }) {
  const { t } = useTranslation();
  const step = RUNTIME_CONNECT_STAGES.indexOf(stage);
  const total = RUNTIME_CONNECT_STAGES.length;
  return (
    // No aria-busy: it would hold back the progress bar's step announcements.
    <main className={styles.screen}>
      <section className={styles.content} aria-labelledby="runtime-title">
        <Stack gap="wide" align="center">
          <BrandIdentity size="standard" />
          <Text as="h1" kind="heading" id="runtime-title">
            {t('runtimeConnection.connecting.title')}
          </Text>
          {/* The bar fills with finished steps; the text names the step under way. */}
          <ProgressBar
            label={t(`runtimeConnection.stages.${stage}`)}
            value={step}
            maxValue={total}
            valueLabel={t('runtimeConnection.step', { current: step + 1, total })}
          />
        </Stack>
      </section>
    </main>
  );
}

function ConnectionScreen({
  state,
  stage,
  onInstall,
  onRetry,
}: {
  state: Exclude<RuntimeStatus, 'ready'> | 'connecting';
  stage: RuntimeConnectStage;
  onInstall?: () => void;
  onRetry: () => Promise<void>;
}) {
  const { t } = useTranslation();
  if (state === 'connecting') return <ConnectingScreen stage={stage} />;
  return (
    <main className={styles.screen}>
      <section className={styles.content} aria-labelledby="runtime-title">
        <Stack gap="wide" align="center">
          <BrandIdentity size="standard" />
          <Stack gap="regular" align="center">
            <Text as="h1" kind="heading" id="runtime-title">
              {t(`runtimeConnection.${state}.title`)}
            </Text>
            <div role="alert">
              <Text tone="muted">{t(`runtimeConnection.${state}.description`)}</Text>
            </div>
          </Stack>
          <Inline justify="center" gap="section">
            {onInstall && (state === 'not_installed' || state === 'incompatible') ? (
              <ActionButton onClick={onInstall}>
                {t(
                  state === 'incompatible'
                    ? 'runtimeConnection.update'
                    : 'runtimeConnection.install',
                )}
              </ActionButton>
            ) : null}
            <ActionButton onClick={() => void onRetry()}>
              {t('runtimeConnection.retry')}
            </ActionButton>
          </Inline>
        </Stack>
      </section>
    </main>
  );
}
