import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import '../../test-support/integration';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { processorReportState } from '../../../.storybook/fixtures/processorReport';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import {
  advance,
  eventually,
  find,
  finishFakeTimerTest,
} from '../../test-support/fakeTimerQueries';
import { RoutingWorkspace } from './RoutingWorkspace';

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(228);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(120);
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(private callback: ResizeObserverCallback) {}
      observe(target: Element) {
        queueMicrotask(() =>
          this.callback(
            [
              {
                target,
                borderBoxSize: [],
                contentBoxSize: [],
                devicePixelContentBoxSize: [],
                contentRect: new DOMRect(0, 0, 228, 120),
              },
            ],
            this,
          ),
        );
      }
      unobserve() {}
      disconnect() {}
    },
  );
});

afterEach(() => finishFakeTimerTest(() => act(async () => vi.runOnlyPendingTimersAsync())));

it('shows processing failure and recovery on the affected node while the other node remains available', async () => {
  vi.useFakeTimers();
  await i18n.changeLanguage('en');
  const state = processorReportState({ failed: true });
  const controlPort = new StorybookMockControlPort(state);
  const view = render(
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <RoutingWorkspace
          controlPort={controlPort}
          graphId="graph-1"
          catalogRevision={1}
          failedProcessors={state.status.failedProcessors}
        />
      </MixRelayProvider>
    </I18nextProvider>,
  );
  await eventually(() =>
    expect(screen.getByRole('group', { name: 'Voice effect' })).toHaveTextContent(
      'Plug-in processing stopped',
    ),
  );
  const voice = screen.getByRole('group', { name: 'Voice effect' });
  await eventually(() =>
    expect(
      within(voice).getByRole('button', { name: 'Retry crashed plug-in: Voice effect' }),
    ).toBeEnabled(),
  );
  expect(screen.getByRole('group', { name: 'Music effect' })).not.toHaveTextContent(
    'Plug-in processing stopped',
  );
  view.rerender(
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <RoutingWorkspace
          controlPort={controlPort}
          graphId="graph-1"
          catalogRevision={1}
          failedProcessors={[]}
        />
      </MixRelayProvider>
    </I18nextProvider>,
  );
  await eventually(() => expect(voice).not.toHaveTextContent('Plug-in processing stopped'));
  view.unmount();
});

it('opens the report from the real processor menu, rescans, and restores keyboard focus without changing audio', async () => {
  vi.useFakeTimers();
  await i18n.changeLanguage('en');
  const state = processorReportState();
  const controlPort = new StorybookMockControlPort(state);
  const scan = vi.spyOn(controlPort, 'rescanProcessors').mockImplementation(async () => {
    if (state.processors) state.processors.diagnostics = [];
    return controlPort.getProcessors();
  });
  const apply = vi.spyOn(controlPort, 'applyGraph');
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  const view = render(
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <RoutingWorkspace controlPort={controlPort} graphId="graph-1" />
      </MixRelayProvider>
    </I18nextProvider>,
  );
  const add = await find('button', { name: 'Add node' });
  await advance();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.click(add);
  const processor = await find('menuitem', { name: 'Processor' });
  processor.focus();
  await user.keyboard('{ArrowRight}');
  const entry = await find('menuitem', { name: 'View search report…' });
  expect(entry).toHaveTextContent('19');
  await user.click(entry);
  const dialog = await find('dialog', { name: 'Effect search report' });
  expect(dialog).toHaveTextContent('Unavailable Effect 19');
  await user.click(within(dialog).getByRole('button', { name: 'Rescan audio effects' }));
  await eventually(() =>
    expect(dialog).toHaveTextContent('Effect search completed without issues.'),
  );
  expect(scan).toHaveBeenCalledExactlyOnceWith([]);
  expect(within(dialog).queryByText('Unavailable Effect 19')).not.toBeInTheDocument();
  await user.keyboard('{Escape}');
  await eventually(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  await eventually(() => expect(add).toHaveFocus());
  expect(apply).not.toHaveBeenCalled();
  view.unmount();
});

it('keeps a failed catalog lookup in the report and retries that lookup without editing audio', async () => {
  vi.useFakeTimers();
  await i18n.changeLanguage('en');
  const controlPort = new StorybookMockControlPort(processorReportState());
  const lookup = vi
    .spyOn(controlPort, 'getProcessors')
    .mockRejectedValueOnce(new Error('unavailable'));
  const scan = vi.spyOn(controlPort, 'rescanProcessors');
  const apply = vi.spyOn(controlPort, 'applyGraph');
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  const view = render(
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <RoutingWorkspace controlPort={controlPort} graphId="graph-1" />
      </MixRelayProvider>
    </I18nextProvider>,
  );
  await find('button', { name: 'Add node' });
  await advance();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Add node' }));
  const processor = await find('menuitem', { name: 'Processor' });
  processor.focus();
  await user.keyboard('{ArrowRight}');
  const entry = await find('menuitem', { name: 'View search report…' });
  expect(entry).toHaveTextContent('Effect search incomplete');
  await user.click(entry);
  const dialog = await find('dialog', { name: 'Effect search report' });
  expect(dialog).toHaveTextContent(
    'Could not load audio effects. The last available list is preserved.',
  );
  await user.click(within(dialog).getByRole('button', { name: 'Retry' }));
  await eventually(() => expect(dialog).toHaveTextContent('Scan issues (19)'));
  expect(lookup).toHaveBeenCalledTimes(2);
  expect(scan).not.toHaveBeenCalled();
  expect(apply).not.toHaveBeenCalled();
  view.unmount();
});

it('puts an editor communication failure in its own node and keeps the confirmed state until explicit retry succeeds', async () => {
  vi.useFakeTimers();
  await i18n.changeLanguage('en');
  const controlPort = new StorybookMockControlPort(processorReportState({ editor: true }));
  const getEditor = vi
    .spyOn(controlPort, 'getProcessorEditor')
    .mockImplementation(async (nodeId) => {
      if (nodeId === 'voice') throw new Error('unavailable');
      return {
        protocolVersion: 1,
        nodeId,
        state: 'open',
        failure: null,
        width: null,
        height: null,
        resizable: false,
      };
    });
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  const view = render(
    <I18nextProvider i18n={i18n}>
      <MixRelayProvider>
        <RoutingWorkspace controlPort={controlPort} graphId="graph-1" />
      </MixRelayProvider>
    </I18nextProvider>,
  );
  const alert = await find('alert');
  expect(alert).toHaveTextContent('Voice effect');
  const node = await find('group', { name: 'Voice effect' });
  expect(node).toContainElement(alert);
  const music = screen.getByRole('group', { name: 'Music effect' });
  expect(within(music).queryByRole('alert')).not.toBeInTheDocument();
  getEditor.mockImplementation(async (nodeId) => ({
    protocolVersion: 1,
    nodeId,
    state: 'open',
    failure: null,
    width: null,
    height: null,
    resizable: false,
  }));
  await user.click(within(node).getByRole('button', { name: 'Retry' }));
  await eventually(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  view.unmount();
  await act(async () => vi.runOnlyPendingTimersAsync());
});
