import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ProductUpdate, UpdateDownload, UpdateProduct } from '../../core/domain/updates';
import type { UpdatesPort } from '../../core/ports/UpdatesPort';
import { ActionButton } from '../../core/ui/ActionButton';
import { ContentSection } from '../../core/ui/ContentSection';
import { FormActions } from '../../core/ui/FormActions';
import { IconActionButton } from '../../core/ui/IconActionButton';
import { Inline } from '../../core/ui/Inline';
import { InlineNotice } from '../../core/ui/InlineNotice';
import type { StatusTone } from '../../core/ui/StatusBadge';
import { Text } from '../../core/ui/Text';
import { UpdatesDialog } from '../../core/ui/UpdatesDialog';
import { useUpdatesSession } from './useUpdatesSession';

const products: UpdateProduct[] = ['mix-relay'];
const updateTones = {
  current: 'positive',
  unavailable: 'critical',
  available: 'accent',
  published: 'accent',
  not_published: 'neutral',
} satisfies Record<ProductUpdate['state'], StatusTone>;

function downloadLabel(download: UpdateDownload) {
  const extension = new URL(download.url).pathname.match(/\.(pkg|dmg|deb|rpm|appimage)$/i)?.[1];
  return `${extension ? extension.toUpperCase() : download.os} · ${download.arch}`;
}

function ProductRelease({
  product,
  update,
  session,
}: {
  product: UpdateProduct;
  update: ProductUpdate | undefined;
  session: ReturnType<typeof useUpdatesSession>;
}) {
  const { t } = useTranslation();
  const state = session.failed ? 'unavailable' : (update?.state ?? 'unavailable');
  const downloads =
    state === 'unavailable' || state === 'not_published' ? [] : (update?.downloads ?? []);
  return (
    <ContentSection title="MixRelay" divider="after">
      <Text kind="supporting" tone="muted">
        {t('updates.installed', { version: update?.installedVersion ?? t('updates.unknown') })}
      </Text>
      {update?.latestVersion && update.latestVersion !== update.installedVersion ? (
        <Text kind="supporting" tone="muted">
          {t('updates.latest', { version: update.latestVersion })}
        </Text>
      ) : null}
      {session.checked ? (
        <Text kind="supporting" tone={updateTones[state]}>
          {t(`updates.states.${state}`)}
        </Text>
      ) : null}
      <Inline>
        {downloads.map((download) => (
          <ActionButton
            key={download.url}
            disabled={session.checking || session.opening !== null}
            onClick={() => void session.openDownload(product, download.url)}
          >
            {t('updates.openDownload', { asset: downloadLabel(download) })}
          </ActionButton>
        ))}
      </Inline>
    </ContentSection>
  );
}

/** Update checks and downloads remain available during runtime recovery. */
export function UpdateAccess({
  children,
  port,
}: {
  children: (action: ReactNode, openDriverInstaller: (() => void) | undefined) => ReactNode;
  port: UpdatesPort | null;
}) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState(false);
  const session = useUpdatesSession(port);
  const available = session.products.some((product) => product.state === 'available');
  const actionLabel = t(available ? 'updates.available' : 'updates.check');
  const action = port ? (
    <IconActionButton
      label={actionLabel}
      indicator={available}
      aria-expanded={opened}
      aria-haspopup="dialog"
      onClick={() => {
        setOpened(true);
        void session.refresh();
      }}
    >
      <svg aria-hidden="true" viewBox="0 0 16 16">
        <path d="M8 2v8m-3-3 3 3 3-3M3 11v3h10v-3" />
      </svg>
    </IconActionButton>
  ) : null;

  return (
    <>
      {children(
        action,
        port
          ? () => {
              setOpened(true);
              void session.refresh();
            }
          : undefined,
      )}
      <UpdatesDialog
        opened={opened && port !== null}
        onClose={() => setOpened(false)}
        closeLabel={t('updates.close')}
        title={t('updates.title')}
      >
        {session.checking ? (
          <div role="status">
            <Text kind="supporting">{t('updates.checking')}</Text>
          </div>
        ) : null}
        {products.map((product) => (
          <ProductRelease
            key={product}
            product={product}
            session={session}
            update={session.products.find((update) => update.product === product)}
          />
        ))}
        {session.openFailed ? (
          <InlineNotice tone="critical">{t('updates.openFailed')}</InlineNotice>
        ) : null}
        <Text kind="supporting" tone="muted">
          {t('updates.installNotice')}
        </Text>
        <FormActions>
          <ActionButton
            disabled={session.checking || session.opening !== null}
            onClick={() => void session.refresh()}
          >
            {t('updates.checkAgain')}
          </ActionButton>
        </FormActions>
      </UpdatesDialog>
    </>
  );
}
