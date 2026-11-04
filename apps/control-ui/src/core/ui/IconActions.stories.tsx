import type { Meta, StoryObj } from '@storybook/react-vite';
import { useTranslation } from 'react-i18next';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { i18n } from '../i18n';
import { AppInformationButton } from './AppInformationButton';
import { AudioNodeCard } from './AudioNodeCard';
import { CollectionItem } from './CollectionItem';
import styles from './ComponentCatalog.module.css';
import { IconActionButton } from './IconActionButton';
import { NodeOptionsSection } from './NodeOptionsSection';
import { NodeMuteButton, NodePowerButton } from './NodeSignalControls';
import { PathField } from './PathField';
import { StateButton } from './StateButton';
import { ThemePicker } from './ThemePicker';
import { TransportButton } from './TransportButton';

function IconActions({ disabled, onAction }: { disabled: boolean; onAction: () => void }) {
  const { t } = useTranslation();
  return (
    <div className={styles.stack}>
      <section aria-label="Action controls" className={styles.row}>
        <IconActionButton label={t('studio.newGraph')} disabled={disabled} onClick={onAction}>
          +
        </IconActionButton>
        <NodePowerButton
          enabled
          disabled={disabled}
          label={t('routing.nodeControl.powerOff', { name: 'Music' })}
          onChange={onAction}
        />
        <NodeMuteButton
          muted={false}
          disabled={disabled}
          label={t('routing.nodeControl.mute', { name: 'Music' })}
          onChange={onAction}
        />
        <NodeOptionsSection
          label={`Music: ${t('routing.nodes.options')}`}
          opened={false}
          onToggle={onAction}
        >
          Settings
        </NodeOptionsSection>
        <StateButton
          states={[{ value: 'paused', label: t('audioSession.resume'), icon: '▷' }]}
          value="paused"
          disabled={disabled}
          onClick={onAction}
        />
        <AppInformationButton
          label={t('about.title')}
          version="0.1.3"
          opened={false}
          onOpen={onAction}
        />
        {(['record', 'pause', 'stop', 'resume'] as const).map((icon) => (
          <TransportButton
            key={icon}
            icon={icon}
            label={`Music: ${t(`recorder.${icon}`)}`}
            disabled={disabled}
            onClick={onAction}
          />
        ))}
      </section>
      <section aria-label="Graph actions" id="audit-graphs">
        <CollectionItem
          name="Music"
          selected={false}
          disabled={disabled}
          removeLabel={t('studio.deleteGraph')}
          onSelect={onAction}
          onRemove={onAction}
        />
        <CollectionItem
          name="Voice"
          selected={false}
          disabled={disabled}
          removeLabel={t('studio.deleteGraph')}
          onSelect={onAction}
          onRemove={onAction}
        />
      </section>
      <FieldAndNodeActions disabled={disabled} onAction={onAction} />
      <button type="button">Next</button>
    </div>
  );
}

function FieldAndNodeActions({ disabled, onAction }: { disabled: boolean; onAction: () => void }) {
  const { t } = useTranslation();
  return (
    <>
      <section aria-label="Field actions" className={styles.stack}>
        <PathField
          label={t('recorder.folder')}
          value="/Music"
          actionLabel={`Music: ${t('recorder.chooseFolder')}`}
          disabled={disabled}
          onChoose={onAction}
        />
        <ThemePicker />
      </section>
      <section aria-label="Node actions" className={styles.row}>
        <AudioNodeCard
          title="Music"
          icon="FX"
          category="Processor"
          inputCount={2}
          outputCount={2}
          parameterControls={[
            {
              id: 'bypass',
              label: 'Bypass',
              value: 0,
              minimum: 0,
              maximum: 1,
              step: 1,
              suffix: '',
            },
          ]}
          onParameterChange={onAction}
          poweredOff={disabled}
        />
        <AudioNodeCard
          title="Voice"
          icon="FX"
          category="Processor"
          inputCount={2}
          outputCount={2}
          processorEditor={{
            state: 'closed',
            actionLabel: t('routing.processorEditor.open'),
          }}
          onProcessorEditorRetry={onAction}
          poweredOff={disabled}
        />
      </section>
    </>
  );
}

const meta = {
  title: 'Core/Composition/Icon Actions',
  component: IconActions,
  args: { disabled: false, onAction: fn() },
  parameters: {
    docs: {
      description: {
        component:
          '실제 Core 조작의 이름·단일 Tab 소유자·Enter/Space를 검사합니다. callback은 로컬 기록이며 실제 제품 명령을 실행하지 않습니다. Disabled는 비활성 계약이 있는 조작만 잠그며 테마·정보·사이드바는 유지합니다.',
      },
    },
  },
} satisfies Meta<typeof IconActions>;
export default meta;
type Story = StoryObj<typeof meta>;
export const KeyboardOwners: Story = {
  play: async ({ canvasElement, args }) => {
    const surface = within(canvasElement);
    const groups = ['Action controls', 'Graph actions', 'Node actions'];
    const buttons = groups.flatMap((name) =>
      within(surface.getByRole('region', { name })).getAllByRole('button'),
    );
    for (const [index, button] of buttons.entries()) {
      await expect(button).toHaveAccessibleName();
      button.focus();
      await userEvent.keyboard('{Enter}');
      await expect(args.onAction).toHaveBeenCalledTimes(index * 2 + 1);
      await userEvent.keyboard(' ');
      await expect(args.onAction).toHaveBeenCalledTimes(index * 2 + 2);
      await userEvent.tab();
      await expect(button).not.toHaveFocus();
      await userEvent.tab({ shift: true });
      await expect(button).toHaveFocus();
    }
    const fields = within(surface.getByRole('region', { name: 'Field actions' }));
    const folder = fields.getByRole('button', {
      name: `Music: ${i18n.t('recorder.chooseFolder')}`,
    });
    folder.focus();
    await userEvent.keyboard('{Enter}');
    await userEvent.keyboard(' ');
    await expect(args.onAction).toHaveBeenCalledTimes(buttons.length * 2 + 2);
    const theme = fields
      .getAllByRole('button')
      .find((button) => ['menu', 'true'].includes(button.getAttribute('aria-haspopup') ?? ''));
    if (!theme) throw new Error('Missing theme menu trigger');
    await userEvent.tab();
    await expect(theme).toHaveFocus();
    for (const key of ['{Enter}', ' ']) {
      await userEvent.keyboard(key);
      const menu = await within(canvasElement.ownerDocument.body).findByRole('menu');
      await waitFor(() => expect(menu).toBeVisible());
      await expect(menu).toHaveAccessibleName(theme.getAttribute('aria-label') ?? '');
      await userEvent.keyboard('{Escape}');
      await waitFor(() => expect(theme).toHaveFocus());
    }
  },
};
export const Disabled: Story = { args: { disabled: true } };
export const NarrowLongTranslation: Story = {
  globals: { locale: 'pseudo' },
  parameters: { viewport: { value: 'mobile' } },
};
