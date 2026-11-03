import { readFileSync } from 'node:fs';
import { fireEvent, render, screen } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { mixrelayVariables, themeRoles } from '../theme/tokens';
import { ActionButton } from './ActionButton';
import { CheckboxField } from './CheckboxField';
import { CollectionItem } from './CollectionItem';
import { IconActionButton } from './IconActionButton';
import { PathField } from './PathField';
import { RetryAlert } from './RetryAlert';
import { SidebarToggle } from './SidebarToggle';
import type { StateButton } from './StateButton';
import type { TextField } from './TextField';
import { TitleField } from './TitleField';

it('maps command meaning and density to the shared theme while preserving behavior and disabled state', () => {
  const onClick = vi.fn();
  const view = render(
    <MixRelayProvider>
      <ActionButton onClick={onClick}>Create</ActionButton>
    </MixRelayProvider>,
  );
  const button = screen.getByRole('button', { name: 'Create' });
  expect(button).toHaveAttribute('data-density', 'regular');
  expect(button).toHaveAttribute('data-emphasis', 'quiet');
  fireEvent.click(button);
  expect(onClick).toHaveBeenCalledOnce();
  view.rerender(
    <MixRelayProvider>
      <ActionButton
        tone="critical"
        density="compact"
        emphasis="outlined"
        align="start"
        stretch
        disabled
        loading
        onClick={onClick}
      >
        Remove
      </ActionButton>
    </MixRelayProvider>,
  );
  expect(button).toHaveAttribute('data-density', 'compact');
  expect(button).toHaveAttribute('data-emphasis', 'outlined');
  expect(button).toHaveAttribute('data-tone', 'critical');
  expect(button).toHaveAttribute('data-stretch', 'true');
  expect(button).toBeDisabled();
  fireEvent.click(button);
  expect(onClick).toHaveBeenCalledOnce();
});

it('owns path presentation and provides a separately named destination action', () => {
  const choose = vi.fn();
  const view = render(
    <MixRelayProvider>
      <PathField
        label="Folder"
        value="/recordings"
        actionLabel="Choose folder"
        disabled={false}
        onChoose={choose}
      />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('textbox', { name: 'Folder' })).toHaveAttribute('readonly');
  fireEvent.click(screen.getByRole('button', { name: 'Choose folder' }));
  expect(choose).toHaveBeenCalledOnce();
  view.rerender(
    <MixRelayProvider>
      <PathField label="Folder" value="/recordings" actionLabel="Choose folder" disabled />
    </MixRelayProvider>,
  );
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it('preserves icon action names, state indicators, tones and graph interaction isolation', () => {
  const view = render(
    <IconActionButton label="Retry" aria-busy>
      <span>↻</span>
    </IconActionButton>,
  );
  const button = screen.getByRole('button', { name: 'Retry' });
  expect(button).toHaveClass('nodrag', 'nopan');
  expect(button).toHaveAttribute('aria-busy', 'true');
  expect(button).toHaveAttribute('data-density', 'compact');
  expect(button.querySelector('[aria-hidden]')).toBeNull();
  view.rerender(
    <IconActionButton label="Retry" tone="warning" density="regular" indicator disabled>
      <span>↻</span>
    </IconActionButton>,
  );
  expect(button).toHaveAttribute('data-tone', 'warning');
  expect(button.querySelector('[aria-hidden]')).not.toBeNull();
  expect(button).toBeDisabled();
});

it('retains collection selection, removal, title editing and sidebar ownership', () => {
  const select = vi.fn();
  const remove = vi.fn();
  const toggle = vi.fn();
  const view = render(
    <>
      <CollectionItem
        name="Studio"
        selected={false}
        disabled={false}
        removeLabel="Delete graph"
        onSelect={select}
        onRemove={remove}
      />
      <TitleField aria-label="Graph name" defaultValue="Studio" />
      <SidebarToggle label="Graphs" opened controls="graph-sidebar" onToggle={toggle} />
    </>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Studio' }));
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Delete graph') }));
  fireEvent.click(screen.getByRole('button', { name: 'Graphs' }));
  expect(select).toHaveBeenCalledOnce();
  expect(remove).toHaveBeenCalledOnce();
  expect(toggle).toHaveBeenCalledOnce();
  expect(screen.getByRole('textbox', { name: 'Graph name' })).toHaveValue('Studio');
  view.rerender(
    <>
      <CollectionItem
        name="Studio"
        selected
        disabled
        removeLabel="Delete graph"
        onSelect={select}
        onRemove={remove}
      />
      <SidebarToggle label="Graphs" opened={false} controls="graph-sidebar" onToggle={toggle} />
    </>,
  );
  expect(screen.getByRole('button', { name: 'Studio' })).toHaveAttribute('aria-current', 'page');
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Delete graph') }),
  ).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Graphs' })).toHaveAttribute('aria-expanded', 'false');
});

it('keeps checkbox confirmation and retry as real controls', () => {
  const change = vi.fn();
  const retry = vi.fn();
  const view = render(
    <MixRelayProvider>
      <CheckboxField label="Confirm removal" checked={false} onChange={change} />
      <RetryAlert message="Failed" retryLabel="Retry" busy={false} onRetry={retry} />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Confirm removal' }));
  expect(change).toHaveBeenCalledWith(true);
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  expect(retry).toHaveBeenCalledOnce();
  view.rerender(
    <MixRelayProvider>
      <RetryAlert message="Failed" retryLabel="Retry" busy onRetry={retry} />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
});

it('rejects styling hidden in typed spreads for public controls', () => {
  const override = { style: { color: 'var(--status-danger)' }, disabled: false };
  // @ts-expect-error A spread cannot introduce a style override.
  const command: ComponentProps<typeof ActionButton> = override;
  // @ts-expect-error TextField only exposes functional input props.
  const field: ComponentProps<typeof TextField> = { classNames: { input: 'override' } };
  const overrideState = { states: [], value: 'idle', className: 'override' };
  // @ts-expect-error StateButton cannot accept a caller className.
  const state: ComponentProps<typeof StateButton> = overrideState;
  expect(command).toBe(override);
  expect(field).toHaveProperty('classNames');
  expect(state).toHaveProperty('className');
});

it('persistent MIDI toggle fill is visually distinct from mappable target overlays', () => {
  const style = document.createElement('style');
  const variables: Record<string, string> = { ...mixrelayVariables, ...themeRoles.light };
  // jsdom does not resolve custom properties in background shorthands.
  let resolvedCss = readFileSync('src/core/theme/ComponentTheme.module.css', 'utf8');
  for (let depth = 0; depth < 4; depth++) {
    resolvedCss = resolvedCss.replace(
      /var\((--[\w-]+)\)/g,
      (original, key: string) => variables[key] ?? original,
    );
  }
  style.textContent = resolvedCss;
  document.head.append(style);
  try {
    render(
      <MixRelayProvider>
        <ActionButton data-midi-toggle aria-pressed="true">
          MIDI
        </ActionButton>
        <ActionButton data-midi-map aria-pressed="false">
          Map volume
        </ActionButton>
      </MixRelayProvider>,
    );
    // CSS modules rename this class in tests; use the source selector for the stylesheet fixture.
    for (const button of screen.getAllByRole('button')) button.classList.add('command');
    const toggle = getComputedStyle(screen.getByRole('button', { name: 'MIDI' }));
    const target = getComputedStyle(screen.getByRole('button', { name: 'Map volume' }));
    expect(toggle.backgroundColor).not.toBe('rgba(0, 0, 0, 0)');
    expect(toggle.backgroundColor).not.toBe(target.backgroundColor);
    expect(toggle.color).not.toBe(target.color);
  } finally {
    style.remove();
  }
});
