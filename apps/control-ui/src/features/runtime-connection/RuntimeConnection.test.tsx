import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RuntimeConnectStage, RuntimeStatus } from '../../core/domain/runtime';
import { I18nProvider, i18n } from '../../core/i18n';
import type { RuntimePort } from '../../core/ports/RuntimePort';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { advance, eventually, find } from '../../test-support/fakeTimerQueries';
import { RuntimeConnection } from './RuntimeConnection';

function renderConnection(port: RuntimePort, install = vi.fn()) {
  return render(
    <MixRelayProvider>
      <I18nProvider>
        <RuntimeConnection port={port} onInstall={install}>
          <label>
            Unsaved name
            <input defaultValue="Live routing" />
          </label>
        </RuntimeConnection>
      </I18nProvider>
    </MixRelayProvider>,
  );
}

describe('Driver connection steps', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en');
  });

  it('shows each native connection step and mounts the editor only after readiness', async () => {
    let finish: (status: RuntimeStatus) => void = () => {};
    let report: (stage: RuntimeConnectStage) => void = () => {};
    const connect = vi.fn(
      (onStage: (stage: RuntimeConnectStage) => void) =>
        new Promise<RuntimeStatus>((resolve) => {
          report = onStage;
          finish = resolve;
        }),
    );
    renderConnection({ connect, status: vi.fn() });
    // A busy region would hold back the step announcements.
    expect(screen.getByRole('main')).not.toHaveAttribute('aria-busy');
    expect(screen.getByRole('heading', { name: 'Connecting to MixRelay' })).toBeVisible();
    const step = (name: string) => screen.getByRole('progressbar', { name });
    expect(step('Checking for a running MixRelay')).toHaveAttribute(
      'aria-valuetext',
      'Step 1 of 3',
    );
    expect(step('Checking for a running MixRelay')).toHaveAttribute('aria-valuenow', '0');
    expect(screen.getByRole('status')).toHaveTextContent('Checking for a running MixRelay');
    await act(async () => report('starting'));
    expect(step('Starting MixRelay')).toHaveAttribute('aria-valuetext', 'Step 2 of 3');
    expect(screen.getByRole('status')).toHaveTextContent('Starting MixRelay');
    expect(step('Starting MixRelay')).toHaveAttribute('aria-valuenow', '1');
    await act(async () => report('preparing_engine'));
    expect(step('Preparing the audio engine')).toHaveAttribute('aria-valuetext', 'Step 3 of 3');
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    await act(async () => {
      finish('ready');
    });
    expect(screen.getByRole('textbox')).toHaveValue('Live routing');
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  });
});

describe('Driver connection gate', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en');
  });

  it.each([
    ['not_installed', 'MixRelay is required', 'Install MixRelay'],
    ['incompatible', 'Update MixRelay', 'Update MixRelay'],
  ] as const)(
    'provides installation guidance for %s and only retries explicitly',
    async (status, title, action) => {
      const connect = vi
        .fn<RuntimePort['connect']>()
        .mockResolvedValueOnce(status)
        .mockResolvedValue('ready');
      const install = vi.fn();
      renderConnection({ connect, status: vi.fn() }, install);
      expect(await screen.findByRole('heading', { name: title })).toBeVisible();
      expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: action }));
      expect(install).toHaveBeenCalledOnce();
      expect(connect).toHaveBeenCalledOnce();
      fireEvent.click(screen.getByRole('button', { name: 'Retry connection' }));
      expect(await screen.findByRole('textbox')).toBeVisible();
      expect(connect).toHaveBeenCalledTimes(2);
    },
  );

  it('ends a failed startup without exposing native errors', async () => {
    renderConnection({
      connect: vi.fn().mockRejectedValue(new Error('private socket error')),
      status: vi.fn(),
    });
    expect(
      await screen.findByRole('heading', { name: 'Cannot connect to MixRelay' }),
    ).toBeVisible();
    expect(screen.queryByText('private socket error')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry connection' })).toBeEnabled();
  });

  it('exposes the startup error and Retry while observing a late-ready engine', async () => {
    vi.useFakeTimers();
    const connect = vi.fn<RuntimePort['connect']>().mockResolvedValue('unavailable');
    const status = vi.fn<RuntimePort['status']>().mockResolvedValue('ready');
    const view = renderConnection({ connect, status });
    try {
      await advance();
      expect(screen.getByRole('heading', { name: 'Cannot connect to MixRelay' })).toBeVisible();
      expect(screen.getByRole('alert')).toHaveTextContent('MixRelay could not be reached');
      expect(screen.getByRole('button', { name: 'Retry connection' })).toBeEnabled();
      expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
      await advance(2000);
      expect(screen.getByRole('textbox')).toBeVisible();
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(connect).toHaveBeenCalledOnce();
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('preserves editor state on disconnect and never automatically restarts stopped audio', async () => {
    // The two-second readiness re-check runs on the fake clock instead of wall time.
    vi.useFakeTimers();
    try {
      const connect = vi.fn<RuntimePort['connect']>().mockResolvedValue('ready');
      const status = vi.fn<RuntimePort['status']>().mockResolvedValue('unavailable');
      renderConnection({ connect, status });
      const input = await find('textbox');
      fireEvent.change(input, { target: { value: 'Unsaved edit' } });
      await eventually(
        () =>
          expect(screen.getByRole('heading', { name: 'Cannot connect to MixRelay' })).toBeVisible(),
        { timeout: 4000 },
      );
      expect(input).not.toBeVisible();
      expect(connect).toHaveBeenCalledOnce();
      fireEvent.click(screen.getByRole('button', { name: 'Retry connection' }));
      expect(await find('textbox')).toHaveValue('Unsaved edit');
    } finally {
      vi.useRealTimers();
    }
  });

  it('ignores a completed connection after unmount', async () => {
    let finish: (status: RuntimeStatus) => void = () => {};
    const view = renderConnection({
      connect: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
      status: vi.fn(),
    });
    view.unmount();
    await act(async () => {
      finish('ready');
    });
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  });
});
