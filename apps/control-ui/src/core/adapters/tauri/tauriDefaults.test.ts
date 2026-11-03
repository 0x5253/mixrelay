import { expect, it, vi } from 'vitest';

const native = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue('ready'),
  listen: vi.fn().mockResolvedValue(() => undefined),
  getVersion: vi.fn().mockResolvedValue('1.0.0'),
  setTheme: vi.fn().mockResolvedValue(undefined),
  Channel: class {
    constructor(readonly onmessage: (message: unknown) => void) {}
  },
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: native.invoke, Channel: native.Channel }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: native.getVersion }));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ setTheme: native.setTheme }),
}));

const { TauriNativeShell } = await import('./TauriNativeShell');
const { TauriRuntimePort } = await import('./TauriRuntimePort');

it('the packaged shell uses the Tauri app, event, command and window APIs', async () => {
  const shell = new TauriNativeShell();
  await expect(shell.version()).resolves.toBe('1.0.0');
  const listener = vi.fn();
  await shell.onAbout(listener);
  native.listen.mock.calls[0]?.[1]({ payload: null });
  expect(listener).toHaveBeenCalledOnce();
  await shell.setMenuLocale('ja');
  expect(native.invoke).toHaveBeenCalledWith('set_native_menu_locale', { locale: 'ja' });
  await shell.setWindowTheme('system');
  expect(native.setTheme).toHaveBeenCalledWith(null);
});

it('the packaged runtime reports connection progress over a Tauri channel', async () => {
  const onStage = vi.fn();
  await expect(new TauriRuntimePort().connect(onStage)).resolves.toBe('ready');
  const [, args] = native.invoke.mock.lastCall ?? [];
  expect(args.onStage).toBeInstanceOf(native.Channel);
  args.onStage.onmessage('starting');
  expect(onStage).toHaveBeenCalledWith('starting');
});
