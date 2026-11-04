import { render, screen, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { userEvent } from 'storybook/test';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { I18nProvider, i18n } from '../i18n';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AppInformationButton } from './AppInformationButton';
import { AudioNodeCard } from './AudioNodeCard';
import { CollectionItem } from './CollectionItem';
import { Dialog } from './Dialog';
import { NodeOptionsSection } from './NodeOptionsSection';
import { NodeMuteButton, NodePowerButton } from './NodeSignalControls';
import { PathField } from './PathField';
import { SidebarToggle } from './SidebarToggle';
import { StateButton } from './StateButton';
import { ThemePicker } from './ThemePicker';
import { TransportButton } from './TransportButton';

const node = { title: 'Music', icon: 'FX', category: 'Processor', inputCount: 2, outputCount: 2 };
const cases: { name: string; render: (action: () => void) => ReactElement }[] = [
  {
    name: 'Disable Music',
    render: (action) => (
      <NodePowerButton enabled disabled={false} label="Disable Music" onChange={action} />
    ),
  },
  {
    name: 'Mute Music',
    render: (action) => (
      <NodeMuteButton muted={false} disabled={false} label="Mute Music" onChange={action} />
    ),
  },
  {
    name: 'Graphs',
    render: (action) => <SidebarToggle label="Graphs" opened controls="graphs" onToggle={action} />,
  },
  {
    name: 'Music: Delete',
    render: (action) => (
      <CollectionItem
        name="Music"
        selected={false}
        disabled={false}
        removeLabel="Delete"
        onSelect={vi.fn()}
        onRemove={action}
      />
    ),
  },
  {
    name: 'Music: Folder',
    render: (action) => (
      <PathField
        label="Folder"
        value="/Music"
        actionLabel="Music: Folder"
        disabled={false}
        onChoose={action}
      />
    ),
  },
  {
    name: 'Music: Options',
    render: (action) => (
      <NodeOptionsSection label="Music: Options" opened={false} onToggle={action}>
        Settings
      </NodeOptionsSection>
    ),
  },
  {
    name: 'Music: Bypass: 0',
    render: (action) => (
      <AudioNodeCard
        {...node}
        onParameterChange={action}
        parameterControls={[
          { id: 'bypass', label: 'Bypass', minimum: 0, maximum: 1, step: 1, value: 0, suffix: '' },
        ]}
      />
    ),
  },
  {
    name: 'Music: Open editor',
    render: (action) => (
      <AudioNodeCard
        {...node}
        processorEditor={{ state: 'closed', actionLabel: 'Open editor' }}
        onProcessorEditorRetry={action}
      />
    ),
  },
  {
    name: 'About MixRelay',
    render: (action) => (
      <AppInformationButton label="About MixRelay" version="0.1" opened={false} onOpen={action} />
    ),
  },
  {
    name: 'Resume',
    render: (action) => (
      <StateButton
        states={[{ value: 'paused', label: 'Resume', icon: '▷' }]}
        value="paused"
        onClick={action}
      />
    ),
  },
  ...(['record', 'pause', 'stop', 'resume'] as const).map((icon) => ({
    name: `Music: ${icon}`,
    render: (action: () => void) => (
      <TransportButton icon={icon} label={`Music: ${icon}`} disabled={false} onClick={action} />
    ),
  })),
];

beforeEach(async () => {
  await i18n.changeLanguage('en');
  // jsdom rejects the Vitest Window proxy when React Aria synthesizes a MouseEvent
  // from a KeyboardEvent. Only normalize that platform field; keep key handlers real.
  vi.stubGlobal(
    'MouseEvent',
    class extends MouseEvent {
      constructor(type: string, init: MouseEventInit = {}) {
        super(type, { ...init, view: null });
      }
    },
  );
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each(cases)('$name runs once per Enter/Space and has no wrapper Tab stop', async (entry) => {
  const action = vi.fn();
  render(
    <MixRelayProvider>
      <I18nProvider>
        {entry.render(action)}
        <button type="button">Next</button>
      </I18nProvider>
    </MixRelayProvider>,
  );
  const user = userEvent.setup({ delay: null });
  const target = screen.getByRole('button', { name: entry.name });
  target.focus();
  await user.keyboard('{Enter}');
  expect(action).toHaveBeenCalledTimes(1);
  await user.keyboard(' ');
  expect(action).toHaveBeenCalledTimes(2);
  await user.tab();
  expect(screen.getByRole('button', { name: 'Next' })).toHaveFocus();
  await user.tab({ shift: true });
  expect(target).toHaveFocus();
});

it('names repeated delete actions by their target and skips disabled graph commands', async () => {
  render(
    <MixRelayProvider>
      <CollectionItem
        name="Music"
        selected
        disabled
        removeLabel="Delete"
        onSelect={vi.fn()}
        onRemove={vi.fn()}
      />
      <CollectionItem
        name="Voice"
        selected={false}
        disabled={false}
        removeLabel="Delete"
        onSelect={vi.fn()}
        onRemove={vi.fn()}
      />
    </MixRelayProvider>,
  );
  const user = userEvent.setup({ delay: null });
  await user.tab();
  expect(screen.getByRole('button', { name: /^Voice$/ })).toHaveFocus();
  await user.tab();
  expect(screen.getByRole('button', { name: 'Voice: Delete' })).toHaveFocus();
  expect(screen.getByRole('button', { name: 'Music: Delete' })).toBeDisabled();
});

it('the icon-only dialog close runs once on Enter and once on Space', async () => {
  const close = vi.fn();
  render(
    <MixRelayProvider>
      <Dialog title="Information" closeLabel="Close" opened onClose={close}>
        <button type="button">Next</button>
      </Dialog>
    </MixRelayProvider>,
  );
  const user = userEvent.setup({ delay: null });
  const target = screen.getByRole('button', { name: 'Close' });
  await user.tab();
  expect(target).toHaveFocus();
  await user.keyboard('{Enter}');
  await user.keyboard(' ');
  expect(close).toHaveBeenCalledTimes(2);
  await user.tab();
  expect(screen.getByRole('button', { name: 'Next' })).toHaveFocus();
});

// Focus restoration and the theme attribute change outside the observed container, so
// waitFor would otherwise only notice them on its 50 ms poll.
const fastPoll = { interval: 5 };

it.each(['{Enter}', ' '] as const)(
  'theme icon opens with %s, selects by keyboard and restores focus on Escape',
  async (key) => {
    localStorage.removeItem('mixrelay.theme');
    render(
      <MixRelayProvider>
        <I18nProvider>
          <ThemePicker />
        </I18nProvider>
      </MixRelayProvider>,
    );
    const user = userEvent.setup({ delay: null });
    const target = screen.getByRole('button');
    target.focus();
    await user.keyboard(key);
    const menu = await screen.findByRole('menu', undefined, fastPoll);
    expect(menu).toBeVisible();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(target).toHaveFocus(), fastPoll);
    await user.keyboard(key);
    await screen.findByRole('menu', undefined, fastPoll);
    await user.keyboard('{Home}{ArrowDown}{Enter}');
    await waitFor(
      () => expect(document.documentElement).toHaveAttribute('data-theme', 'light'),
      fastPoll,
    );
    await waitFor(() => expect(target).toHaveFocus(), fastPoll);
  },
);
