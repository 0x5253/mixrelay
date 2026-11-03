import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { I18nProvider, i18n } from '../../core/i18n';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { WorkspaceFileMenu } from './WorkspaceFileMenu';

function native() {
  return new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
}
function view(
  port: ControlPort,
  run: (action: 'new' | 'open' | 'save' | 'save_as', locale: string) => Promise<void>,
  disabled = false,
) {
  return (
    <MixRelayProvider>
      <I18nProvider>
        <WorkspaceFileMenu controlPort={port} disabled={disabled} run={run} />
      </I18nProvider>
    </MixRelayProvider>
  );
}
afterEach(async () => {
  vi.restoreAllMocks();
  await i18n.changeLanguage('en');
});
it('hides file commands when the connected Driver lacks workspace file support', () => {
  render(view(native(), vi.fn()));
  expect(screen.queryByRole('button', { name: 'File' })).not.toBeInTheDocument();
});
it('unsubscribes a late native listener registration when its UI has already unmounted', async () => {
  const pending = Promise.withResolvers<() => void>();
  const stop = vi.fn();
  const port: ControlPort = Object.assign(native(), {
    workspaceFile: vi.fn(),
    onWorkspaceFile: vi.fn(() => pending.promise),
  });
  const rendered = render(view(port, vi.fn()));
  rendered.unmount();
  await act(async () => pending.resolve(stop));
  expect(stop).toHaveBeenCalledTimes(1);
});
it('logs failed native listener registration without making an audio or file mutation', async () => {
  const failed = new Error('listener unavailable');
  const error = vi.spyOn(console, 'error').mockImplementation(() => {});
  const run = vi.fn();
  const port: ControlPort = Object.assign(native(), {
    workspaceFile: vi.fn(),
    onWorkspaceFile: vi.fn().mockRejectedValue(failed),
  });
  render(view(port, run));
  await act(async () => Promise.resolve());
  expect(error).toHaveBeenCalledWith('Workspace menu listener failed', failed);
  expect(run).not.toHaveBeenCalled();
});
it('accepts native save and open for free users and observes current disabled state and handler', async () => {
  let listener: Parameters<NonNullable<ControlPort['onWorkspaceFile']>>[0] | undefined;
  const port: ControlPort = Object.assign(native(), {
    workspaceFile: vi.fn(),
    onWorkspaceFile: vi.fn(async (value: typeof listener) => {
      listener = value;
      return vi.fn();
    }),
  });
  const first = vi.fn().mockResolvedValue(undefined),
    second = vi.fn().mockResolvedValue(undefined);
  const rendered = render(view(port, first));
  await act(async () => Promise.resolve());
  await act(async () => {
    listener?.('save');
    listener?.('open');
  });
  expect(first.mock.calls).toEqual([
    ['save', 'en'],
    ['open', 'en'],
  ]);
  rendered.rerender(view(port, second, true));
  await act(async () => listener?.('save_as'));
  expect(second).not.toHaveBeenCalled();
  rendered.rerender(view(port, second));
  await act(async () => listener?.('save_as'));
  expect(second).toHaveBeenCalledExactlyOnceWith('save_as', 'en');
});
