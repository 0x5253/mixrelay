import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { I18nProvider, i18n, supportedLocales } from '../../core/i18n';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { WorkspaceFileMenu } from './WorkspaceFileMenu';

function mockPort() {
  return new StorybookMockControlPort({
    graph: { edges: [], nodes: [], revision: 1, schemaVersion: 2 },
    graphStatus: { activeRevision: 1, protocolVersion: 1, submittedRevision: 1 },
    status: {
      bufferFrames: null,
      driverState: 'not_installed',
      overruns: 0,
      protocolVersion: 1,
      sampleRate: null,
      underruns: 0,
    },
  });
}

beforeEach(() => {
  // Floating UI hides detached anchors; jsdom needs a measured viewport and button.
  vi.spyOn(document.documentElement, 'clientWidth', 'get').mockReturnValue(800);
  vi.spyOn(document.documentElement, 'clientHeight', 'get').mockReturnValue(600);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(200);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(120);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(
    new DOMRect(0, 0, 200, 120),
  );
});

afterEach(async () => {
  cleanup();
  vi.restoreAllMocks();
  await i18n.changeLanguage('en');
});

describe.each(supportedLocales)('workspace file menu in %s', (locale) => {
  it('renders translated file actions and sends the resolved language', async () => {
    await i18n.changeLanguage(locale);
    const run = vi.fn().mockResolvedValue(undefined);
    const port = mockPort();
    const controlPort: ControlPort = Object.assign(port, { workspaceFile: vi.fn() });
    render(
      <I18nProvider>
        <MixRelayProvider>
          <WorkspaceFileMenu controlPort={controlPort} disabled={false} run={run} />
        </MixRelayProvider>
      </I18nProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: i18n.t('workspaceFile.file') }));
    const actions = ['new', 'open', 'save', 'saveAs'] as const;
    for (const action of actions)
      await waitFor(() =>
        expect(
          screen.getByRole('menuitem', {
            name: i18n.t(`workspaceFile.${action}`),
          }),
        ).toBeVisible(),
      );
    fireEvent.click(screen.getByRole('menuitem', { name: i18n.t('workspaceFile.saveAs') }));
    expect(run).toHaveBeenCalledWith('save_as', locale);
  });
});

it('keeps the native file callback in sync when the UI language changes', async () => {
  let onAction: Parameters<NonNullable<ControlPort['onWorkspaceFile']>>[0] | undefined;
  const unsubscribe = vi.fn();
  const port = mockPort();
  const controlPort: ControlPort = Object.assign(port, {
    workspaceFile: vi.fn(),
    onWorkspaceFile: vi.fn(async (listener: typeof onAction) => {
      onAction = listener;
      return unsubscribe;
    }),
  });
  const run = vi.fn().mockResolvedValue(undefined);
  const view = render(
    <I18nProvider>
      <MixRelayProvider>
        <WorkspaceFileMenu controlPort={controlPort} disabled={false} run={run} />
      </MixRelayProvider>
    </I18nProvider>,
  );
  await act(async () => {
    await i18n.changeLanguage('de');
  });
  onAction?.('open');
  expect(run).toHaveBeenCalledWith('open', 'de');
  view.unmount();
  expect(unsubscribe).toHaveBeenCalledOnce();
});
