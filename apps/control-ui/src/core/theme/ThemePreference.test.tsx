import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { browserThemePreferenceStorage } from '../adapters/browser/themePreferenceStorage';
import { themePreference, themeStorageKey } from '../domain/appearance';
import { i18n } from '../i18n';
import { ThemePicker } from '../ui/ThemePicker';
import { MixRelayProvider, useTheme } from './MixRelayProvider';
import { themeCss } from './theme';

const browserStorage = browserThemePreferenceStorage();

afterEach(() => {
  localStorage.removeItem(themeStorageKey);
  vi.restoreAllMocks();
});

function Consumer() {
  const { preference, setPreference } = useTheme();
  return (
    <button type="button" onClick={() => setPreference('dark')}>
      {preference}
    </button>
  );
}

it('keeps a standalone consumer neutral when no appearance provider is present', () => {
  render(<Consumer />);
  fireEvent.click(screen.getByRole('button', { name: 'system' }));
  expect(screen.getByRole('button', { name: 'system' })).toBeInTheDocument();
  expect(localStorage.getItem(themeStorageKey)).toBeNull();
});

it('restores an existing host appearance when its provider is removed', () => {
  const root = document.documentElement;
  const previous = root.getAttribute('data-theme');
  root.setAttribute('data-theme', 'light');
  const mounted = render(<MixRelayProvider theme="dark">Preview</MixRelayProvider>);
  expect(root).toHaveAttribute('data-theme', 'dark');
  mounted.unmount();
  expect(root).toHaveAttribute('data-theme', 'light');
  if (previous === null) root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', previous);
});

it('uses system appearance by default and follows OS changes through media CSS', () => {
  localStorage.removeItem(themeStorageKey);
  render(
    <MixRelayProvider store={browserStorage}>
      <Consumer />
    </MixRelayProvider>,
  );
  expect(document.documentElement).toHaveAttribute('data-theme', 'system');
  expect(themeCss).toContain('@media(prefers-color-scheme:dark)');
  expect(themeCss).toContain(':root:not([data-theme="light"])');
});

it('restores, changes and saves a preference across provider remounts', () => {
  localStorage.setItem(themeStorageKey, 'light');
  const first = render(
    <MixRelayProvider store={browserStorage}>
      <Consumer />
    </MixRelayProvider>,
  );
  expect(document.documentElement).toHaveAttribute('data-theme', 'light');
  fireEvent.click(screen.getByRole('button', { name: 'light' }));
  expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  expect(localStorage.getItem(themeStorageKey)).toBe('dark');
  first.unmount();
  render(
    <MixRelayProvider store={browserStorage}>
      <Consumer />
    </MixRelayProvider>,
  );
  expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
});

it('keeps an unsaved workspace edit mounted when appearance changes', () => {
  render(
    <MixRelayProvider store={browserStorage}>
      <Consumer />
      <input aria-label="Draft name" defaultValue="Studio" />
    </MixRelayProvider>,
  );
  const draft = screen.getByRole('textbox', { name: 'Draft name' });
  fireEvent.change(draft, { target: { value: 'Unsaved session' } });
  fireEvent.click(screen.getByRole('button', { name: 'system' }));
  expect(screen.getByRole('textbox', { name: 'Draft name' })).toBe(draft);
  expect(draft).toHaveValue('Unsaved session');
});

it('synchronizes saved preferences and clearing storage without reacting to other settings', () => {
  render(
    <MixRelayProvider store={browserStorage}>
      <Consumer />
    </MixRelayProvider>,
  );
  act(() =>
    window.dispatchEvent(new StorageEvent('storage', { key: themeStorageKey, newValue: 'light' })),
  );
  expect(document.documentElement).toHaveAttribute('data-theme', 'light');
  act(() =>
    window.dispatchEvent(new StorageEvent('storage', { key: 'another.setting', newValue: 'dark' })),
  );
  expect(document.documentElement).toHaveAttribute('data-theme', 'light');
  act(() => window.dispatchEvent(new StorageEvent('storage', { key: null })));
  expect(document.documentElement).toHaveAttribute('data-theme', 'system');
});

it('treats unknown preferences and inaccessible storage as system while allowing session changes', () => {
  expect(themePreference('unexpected')).toBe('system');
  expect(themePreference('system')).toBe('system');
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
    throw new Error('blocked');
  });
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('blocked');
  });
  const storage = browserThemePreferenceStorage();
  expect(storage.read()).toBe('system');
  expect(() => storage.write('dark')).not.toThrow();
  render(
    <MixRelayProvider store={browserStorage}>
      <Consumer />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'system' }));
  expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
});

it('lets Storybook control appearance without changing the saved app choice', () => {
  localStorage.setItem(themeStorageKey, 'light');
  const changed = vi.fn();
  const { rerender } = render(
    <MixRelayProvider theme="system" onThemeChange={changed}>
      <Consumer />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'system' }));
  expect(changed).toHaveBeenCalledWith('dark');
  expect(localStorage.getItem(themeStorageKey)).toBe('light');
  rerender(
    <MixRelayProvider theme="dark">
      <Consumer />
    </MixRelayProvider>,
  );
  expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  fireEvent.click(screen.getByRole('button', { name: 'dark' }));
  expect(localStorage.getItem(themeStorageKey)).toBe('light');
});

it('keeps pseudo appearance labels usable through the shared locale provider', async () => {
  await i18n.changeLanguage('pseudo');
  render(
    <MixRelayProvider store={browserStorage}>
      <ThemePicker />
    </MixRelayProvider>,
  );
  expect(
    screen.getByRole('button', {
      name: i18n.t('theme.current', { theme: i18n.t('theme.system') }),
    }),
  ).toBeInTheDocument();
});

it('exposes one keyboard trigger and checked appearance choices, then returns focus', async () => {
  await i18n.changeLanguage('en');
  localStorage.removeItem(themeStorageKey);
  render(
    <MixRelayProvider store={browserStorage}>
      <ThemePicker />
    </MixRelayProvider>,
  );
  const trigger = screen.getByRole('button', { name: 'Appearance: System' });
  expect(screen.getAllByRole('button')).toHaveLength(1);
  trigger.focus();
  fireEvent.keyDown(trigger, { key: 'Enter', code: 'Enter' });
  fireEvent.keyUp(trigger, { key: 'Enter', code: 'Enter' });
  const light = await screen.findByRole('menuitemradio', { name: 'Light' });
  expect(screen.getByRole('menuitemradio', { name: 'System' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  light.focus();
  fireEvent.keyDown(light, { key: 'Enter', code: 'Enter' });
  fireEvent.keyUp(light, { key: 'Enter', code: 'Enter' });
  await waitFor(() => expect(document.documentElement).toHaveAttribute('data-theme', 'light'));
  await waitFor(() => expect(trigger).toHaveFocus());
  expect(trigger).toHaveAccessibleName('Appearance: Light');
});

it('keeps an uncontrolled preference in the supplied store instead of local storage', () => {
  let saved: 'light' | 'dark' | 'system' = 'dark';
  let changed: ((value: 'light' | 'dark' | 'system') => void) | undefined;
  const store = {
    read: () => saved,
    write: vi.fn((value: typeof saved) => {
      saved = value;
    }),
    subscribe: vi.fn((listener: (value: typeof saved) => void) => {
      changed = listener;
      return () => {
        changed = undefined;
      };
    }),
  };
  const view = render(
    <MixRelayProvider store={store}>
      <Consumer />
    </MixRelayProvider>,
  );
  expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  act(() => changed?.('light'));
  expect(document.documentElement).toHaveAttribute('data-theme', 'light');
  fireEvent.click(screen.getByRole('button', { name: 'light' }));
  expect(store.write).toHaveBeenCalledWith('dark');
  expect(localStorage.getItem(themeStorageKey)).toBeNull();
  view.unmount();
  expect(changed).toBeUndefined();
});

it('keeps an uncontrolled choice for this session only when no store is given', () => {
  localStorage.setItem(themeStorageKey, 'light');
  render(
    <MixRelayProvider>
      <Consumer />
    </MixRelayProvider>,
  );
  expect(document.documentElement).toHaveAttribute('data-theme', 'system');
  fireEvent.click(screen.getByRole('button', { name: 'system' }));
  expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  expect(localStorage.getItem(themeStorageKey)).toBe('light');
});
