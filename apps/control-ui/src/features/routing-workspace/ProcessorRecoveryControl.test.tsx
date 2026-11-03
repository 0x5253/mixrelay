import { act, fireEvent, render, screen } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { I18nextProvider } from 'react-i18next';
import { describe, expect, it, vi } from 'vitest';
import type { FailedProcessorInstance } from '../../core/domain/control/status';

import { i18n } from '../../core/i18n';
import { ProcessorRecoveryControl } from './ProcessorRecoveryControl';

const failed: FailedProcessorInstance = {
  graphId: 'graph-a',
  instanceId: '18446744073709551615',
  nodeId: 'provider-a',
};

function recovery(props: Partial<ComponentProps<typeof ProcessorRecoveryControl>> = {}) {
  return (
    <I18nextProvider i18n={i18n}>
      <ProcessorRecoveryControl
        busy={false}
        catalogRevision={12}
        enabled
        failedProcessors={[failed]}
        graphId="graph-a"
        isProcessor
        nodeId="provider-a"
        nodeName="Crashy"
        onRetry={vi.fn().mockResolvedValue(undefined)}
        supported
        {...props}
      />
    </I18nextProvider>
  );
}

describe('ProcessorRecoveryControl', () => {
  it('shows one explicit crashed plug-in retry with exact opaque identity and revisions', async () => {
    await i18n.changeLanguage('en');
    const onRetry = vi.fn().mockResolvedValue(undefined);
    render(recovery({ onRetry }));

    const button = screen.getByRole('button', { name: 'Retry crashed plug-in: Crashy' });
    expect(button).toBeEnabled();
    expect(button).toHaveAttribute('data-tone', 'critical');
    expect(button).toHaveAttribute('aria-busy', 'false');
    expect(button).not.toHaveAttribute('title');
    await act(async () => fireEvent.click(button));
    expect(onRetry).toHaveBeenCalledExactlyOnceWith({
      expectedCatalogRevision: 12,
      expectedInstanceId: '18446744073709551615',
      graphId: 'graph-a',
      nodeId: 'provider-a',
    });
  });

  it.each([
    ['another selected graph', { graphId: 'graph-b' }],
    ['a deleted node', { nodeId: 'deleted-provider' }],
    ['a non-processor node', { isProcessor: false }],
    ['an unsupported control port', { supported: false }],
    ['a missing catalog revision', { catalogRevision: undefined }],
  ] as const)('hides retry for %s', async (_reason, props) => {
    await i18n.changeLanguage('en');
    render(recovery(props));
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it.each([
    ['an off node', { enabled: false }],
    ['a busy workspace', { busy: true }],
  ] as const)('disables retry for %s', async (_reason, props) => {
    await i18n.changeLanguage('en');
    render(recovery(props));
    const button = screen.getByRole('button', { name: 'Retry crashed plug-in: Crashy' });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute('data-tone', 'critical');
    expect(button).toHaveAttribute('aria-busy', 'false');
  });

  it('shows recovery only while the exact failed instance retry is pending', async () => {
    await i18n.changeLanguage('en');
    const pending = Promise.withResolvers<unknown>();
    const onRetry = vi.fn().mockReturnValue(pending.promise);
    const view = render(recovery({ onRetry }));
    const button = screen.getByRole('button', { name: 'Retry crashed plug-in: Crashy' });
    fireEvent.click(button);
    expect(button).toHaveAttribute('data-tone', 'warning');
    expect(button).toHaveAttribute('aria-busy', 'true');
    expect(button).toBeDisabled();
    view.rerender(recovery({ onRetry, busy: true }));
    await act(async () => pending.resolve(undefined));
    expect(button).toHaveAttribute('data-tone', 'critical');
    expect(button).toHaveAttribute('aria-busy', 'false');
    expect(button).toBeDisabled();
  });

  it('does not label a replacement failed instance as the old instance recovery', async () => {
    await i18n.changeLanguage('en');
    const pending = Promise.withResolvers<unknown>();
    const onRetry = vi.fn().mockReturnValue(pending.promise);
    const view = render(recovery({ onRetry }));
    fireEvent.click(screen.getByRole('button', { name: 'Retry crashed plug-in: Crashy' }));
    view.rerender(
      recovery({ onRetry, failedProcessors: [{ ...failed, instanceId: 'replacement' }] }),
    );
    const button = screen.getByRole('button', { name: 'Retry crashed plug-in: Crashy' });
    expect(button).toHaveAttribute('data-tone', 'critical');
    expect(button).toHaveAttribute('aria-busy', 'false');
    await act(async () => pending.resolve(undefined));
  });

  it('returns to failed after a rejected retry without leaving an unhandled promise', async () => {
    await i18n.changeLanguage('en');
    const pending = Promise.withResolvers<unknown>();
    render(recovery({ onRetry: vi.fn().mockReturnValue(pending.promise) }));
    const button = screen.getByRole('button', { name: 'Retry crashed plug-in: Crashy' });
    fireEvent.click(button);
    expect(button).toHaveAttribute('data-tone', 'warning');
    expect(button).toHaveAttribute('aria-busy', 'true');
    await act(async () => pending.reject(new Error('recovery failed')));
    expect(button).toHaveAttribute('data-tone', 'critical');
    expect(button).toHaveAttribute('aria-busy', 'false');
    expect(button).toBeEnabled();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});

it('keeps a replacement instance retry busy when an older retry settles', async () => {
  await i18n.changeLanguage('en');
  const old = Promise.withResolvers<unknown>();
  const replacement = Promise.withResolvers<unknown>();
  const onRetry = vi.fn().mockReturnValueOnce(old.promise).mockReturnValueOnce(replacement.promise);
  const view = render(recovery({ onRetry }));
  fireEvent.click(screen.getByRole('button', { name: 'Retry crashed plug-in: Crashy' }));
  view.rerender(
    recovery({ onRetry, failedProcessors: [{ ...failed, instanceId: 'replacement' }] }),
  );
  const button = screen.getByRole('button', { name: 'Retry crashed plug-in: Crashy' });
  fireEvent.click(button);
  await act(async () => old.resolve(undefined));
  expect(button).toHaveAttribute('aria-busy', 'true');
  expect(button).toBeDisabled();
  await act(async () => replacement.resolve(undefined));
  expect(button).toHaveAttribute('aria-busy', 'false');
  expect(button).toBeEnabled();
});
