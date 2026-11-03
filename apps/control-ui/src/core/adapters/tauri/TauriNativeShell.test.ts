import { expect, it, vi } from 'vitest';
import { TauriNativeShell } from './TauriNativeShell';

function shell() {
  const api = {
    invoke: vi.fn().mockResolvedValue(undefined),
    listen: vi.fn().mockResolvedValue(vi.fn()),
    version: vi.fn().mockResolvedValue('1.2.3'),
    setTheme: vi.fn().mockResolvedValue(undefined),
  };
  return { api, shell: new TauriNativeShell(api) };
}

it('binds the About and graph list menu events without passing their payloads on', async () => {
  const { api, shell: native } = shell();
  const about = vi.fn();
  const toggle = vi.fn();
  await native.onAbout(about);
  await native.onGraphListToggle(toggle);
  expect(api.listen.mock.calls.map(([event]) => event)).toEqual(['app-about', 'graph-list-toggle']);
  for (const [, listener] of api.listen.mock.calls) listener('payload');
  expect(about).toHaveBeenCalledWith();
  expect(toggle).toHaveBeenCalledWith();
});

it('sends menu language and graph list label commands', async () => {
  const { api, shell: native } = shell();
  await native.setMenuLocale('ko-KR');
  await native.setGraphListVisible(false);
  expect(api.invoke.mock.calls).toEqual([
    ['set_native_menu_locale', { locale: 'ko-KR' }],
    ['set_graph_list_visible', { visible: false }],
  ]);
});

it('releases the window to the operating system for the system appearance', async () => {
  const { api, shell: native } = shell();
  await native.setWindowTheme('dark');
  await native.setWindowTheme('system');
  expect(api.setTheme.mock.calls).toEqual([['dark'], [null]]);
});

it('reads the installed version and refuses one that is not text', async () => {
  const { api, shell: native } = shell();
  await expect(native.version()).resolves.toBe('1.2.3');
  api.version.mockResolvedValueOnce(3);
  await expect(native.version()).rejects.toThrow('app version');
});
