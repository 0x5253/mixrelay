import { expect, it, vi } from 'vitest';
import { themeStorageKey } from '../../domain/appearance';
import { themePreferenceStorage } from './themePreferenceStorage';

function environment(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  const events = new EventTarget();
  return {
    values,
    events,
    storage: () => ({
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => void values.set(key, value),
    }),
  };
}

it('reads and writes the preference under its own key, treating unknown values as system', () => {
  const page = environment({ [themeStorageKey]: 'dark' });
  const store = themePreferenceStorage(page);
  expect(store.read()).toBe('dark');
  store.write('light');
  expect(page.values.get(themeStorageKey)).toBe('light');
  page.values.set(themeStorageKey, 'sepia');
  expect(store.read()).toBe('system');
});

it('reads system and forgets writes while storage is blocked, without throwing', () => {
  const store = themePreferenceStorage({
    storage: () => {
      throw new DOMException('The operation is insecure.', 'SecurityError');
    },
    events: new EventTarget(),
  });
  expect(store.read()).toBe('system');
  expect(() => store.write('dark')).not.toThrow();
});

it('reports only this preference or a full clear from another window, until unsubscribed', () => {
  const page = environment();
  const listener = vi.fn();
  const stop = themePreferenceStorage(page).subscribe(listener);
  const send = (key: string | null, newValue: string | null) =>
    page.events.dispatchEvent(new StorageEvent('storage', { key, newValue }));
  send(themeStorageKey, 'light');
  send('another.setting', 'dark');
  send(null, null);
  expect(listener.mock.calls).toEqual([['light'], ['system']]);
  stop();
  send(themeStorageKey, 'dark');
  expect(listener).toHaveBeenCalledTimes(2);
});

it('gives isolated stores for isolated environments', () => {
  const first = themePreferenceStorage(environment());
  const second = themePreferenceStorage(environment({ [themeStorageKey]: 'dark' }));
  first.write('light');
  expect([first.read(), second.read()]).toEqual(['light', 'dark']);
});
