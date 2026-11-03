import { act, render } from '@testing-library/react';
import { createInstance } from 'i18next';
import type { ComponentProps } from 'react';
import { I18nextProvider } from 'react-i18next';
import { expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { I18nProvider } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import type { HierarchicalMenuButton } from '../../core/ui/HierarchicalMenu';
import { WorkspaceFileMenu } from './WorkspaceFileMenu';

const harness = vi.hoisted(() => ({
  menu: null as ComponentProps<typeof HierarchicalMenuButton> | null,
}));
vi.mock('../../core/ui/HierarchicalMenu', () => ({
  HierarchicalMenuButton: (props: ComponentProps<typeof HierarchicalMenuButton>) => {
    harness.menu = props;
    return null;
  },
}));
function port() {
  return Object.assign(
    new StorybookMockControlPort({
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
    }),
    { workspaceFile: vi.fn() },
  );
}
function menu() {
  if (!harness.menu) throw new Error('Expected a file menu');
  return harness.menu;
}
it('fences unknown and disabled Core menu selections while dispatching every supported native file action', () => {
  const native = port(),
    run = vi.fn().mockResolvedValue(undefined);
  const view = (disabled: boolean) => (
    <MixRelayProvider>
      <I18nProvider>
        <WorkspaceFileMenu controlPort={native} disabled={disabled} run={run} />
      </I18nProvider>
    </MixRelayProvider>
  );
  const rendered = render(view(false));
  act(() => menu().onSelect?.('unknown'));
  expect(run).not.toHaveBeenCalled();
  for (const action of ['new', 'open', 'save', 'save_as']) act(() => menu().onSelect?.(action));
  expect(run.mock.calls.map((call) => call[0])).toEqual(['new', 'open', 'save', 'save_as']);
  rendered.rerender(view(true));
  act(() => menu().onSelect?.('save'));
  expect(run).toHaveBeenCalledTimes(4);
});
it('uses the current language when translations have not resolved a resource locale', async () => {
  const locale = createInstance();
  await locale.init({ lng: 'unlisted', resources: {}, fallbackLng: false });
  expect(locale.resolvedLanguage).toBeUndefined();
  const run = vi.fn().mockResolvedValue(undefined);
  render(
    <MixRelayProvider>
      <I18nextProvider i18n={locale}>
        <WorkspaceFileMenu controlPort={port()} disabled={false} run={run} />
      </I18nextProvider>
    </MixRelayProvider>,
  );
  act(() => menu().onSelect?.('save'));
  expect(run).toHaveBeenCalledExactlyOnceWith('save', 'unlisted');
});
