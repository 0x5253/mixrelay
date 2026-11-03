import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import { expect, it, vi } from 'vitest';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { ProcessorScanReport } from './ProcessorScanReport';
import type { useProcessorCatalog } from './useProcessorCatalog';

function catalog(overrides: Partial<ReturnType<typeof useProcessorCatalog>> = {}) {
  return {
    processors: [],
    diagnostics: [],
    busy: false,
    failure: null,
    refresh: vi.fn(),
    retry: vi.fn(),
    ...overrides,
  };
}
function report(value: ReturnType<typeof useProcessorCatalog>, opened = true, onClose = vi.fn()) {
  return (
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <ProcessorScanReport catalog={value} opened={opened} onClose={onClose} />
      </MixRelayProvider>
    </I18nextProvider>
  );
}

it.each(['en', 'ko-KR', 'pseudo'])(
  'keeps scan issues out of the workspace and reveals technical text explicitly in %s',
  async (locale) => {
    await i18n.changeLanguage(locale);
    const user = userEvent.setup();
    const diagnostic = {
      format: 'vst3' as const,
      stage: 'probe' as const,
      pluginId: 'fixture',
      pluginName: 'Example Effect',
      message: '<script>denied</script>',
    };
    const value = catalog({ diagnostics: [diagnostic, diagnostic] });
    const view = render(report(value, false));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByText('Example Effect')).not.toBeInTheDocument();
    view.rerender(report(value));
    const dialog = screen.getByRole('dialog', {
      name: i18n.t('routing.processorControl.reportTitle'),
    });
    expect(dialog).toHaveTextContent(i18n.t('routing.processorControl.details', { count: 1 }));
    const item = within(dialog).getByRole('button', { name: /Example Effect/ });
    expect(
      within(dialog).getByRole('heading', { level: 3, name: /Example Effect/ }),
    ).toContainElement(item);
    expect(item).toHaveAttribute('aria-expanded', 'false');
    expect(within(dialog).getByText(/<script>denied<\/script>/)).not.toBeVisible();
    item.focus();
    await user.keyboard('{Enter}');
    expect(item).toHaveAttribute('aria-expanded', 'true');
    const panel = within(dialog).getByRole('group', { name: /Example Effect/ });
    expect(panel).toHaveTextContent(i18n.t('routing.processorControl.probeFailed'));
    expect(within(panel).getByText(/<script>denied<\/script>/)).toBeVisible();
    await user.keyboard(' ');
    expect(item).toHaveAttribute('aria-expanded', 'false');
    expect(within(dialog).getByText(/<script>denied<\/script>/)).not.toBeVisible();
  },
);

it.each(['catalog', 'directory', 'paths'] as const)(
  'keeps the %s failure and its explicit retry in the report',
  async (failure) => {
    await i18n.changeLanguage('en');
    const value = catalog({ failure });
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(report(value, true, onClose));
    expect(screen.getByRole('dialog')).toHaveTextContent(
      i18n.t(`routing.processorControl.${failure}`),
    );
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    expect(value.retry).toHaveBeenCalledOnce();
    expect(value.refresh).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledOnce();
  },
);

it('shows scanning without allowing a duplicate request, then can rescan a successful catalog', async () => {
  await i18n.changeLanguage('en');
  const value = catalog({ busy: true });
  const view = render(report(value));
  expect(screen.getByRole('dialog')).toHaveTextContent(i18n.t('routing.processorControl.scanning'));
  expect(screen.getByRole('button', { name: 'Rescan audio effects' })).toBeDisabled();
  view.rerender(report({ ...value, busy: false }));
  expect(screen.getByRole('dialog')).toHaveTextContent(i18n.t('routing.processorControl.complete'));
  await userEvent.setup().click(screen.getByRole('button', { name: 'Rescan audio effects' }));
  expect(value.refresh).toHaveBeenCalledExactlyOnceWith(false);
  expect(value.retry).not.toHaveBeenCalled();
});

it('preserves format-wide scan diagnostics without a plug-in identifier', async () => {
  await i18n.changeLanguage('en');
  render(
    report(
      catalog({
        diagnostics: [{ format: 'clap', stage: 'scan', message: 'Directory access denied' }],
      }),
    ),
  );
  await userEvent.setup().click(screen.getByRole('button', { name: 'CLAP · Discovery' }));
  const panel = screen.getByRole('group', { name: 'CLAP · Discovery' });
  expect(panel).toHaveTextContent(i18n.t('routing.processorControl.scanFailed'));
  expect(within(panel).getByText('Directory access denied')).toBeVisible();
});
