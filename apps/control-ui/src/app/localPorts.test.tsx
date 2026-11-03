import { afterEach, expect, it, vi } from 'vitest';

/** A fresh module graph, so the shared environment ports are created again. */
async function load() {
  vi.resetModules();
  return {
    ...(await import('./localPorts')),
    HttpControlPort: (await import('../core/adapters/control/HttpControlPort')).HttpControlPort,
    TauriControlPort: (await import('../core/adapters/control/TauriControlPort')).TauriControlPort,
    TauriNativeShell: (await import('../core/adapters/tauri/TauriNativeShell')).TauriNativeShell,
  };
}

afterEach(() => {
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
});

it('gives a browser only the engine bridge and no native capability', async () => {
  const { createLocalPorts, HttpControlPort } = await load();
  const ports = createLocalPorts(false);
  expect(ports.controlPort).toBeInstanceOf(HttpControlPort);
  expect(Object.entries(ports).filter(([, port]) => port !== null)).toEqual([
    ['controlPort', ports.controlPort],
  ]);
});

it('gives the packaged app native IPC for every port and its shell', async () => {
  const { createLocalPorts, TauriControlPort, TauriNativeShell } = await load();
  const ports = createLocalPorts(true);
  expect(ports.controlPort).toBeInstanceOf(TauriControlPort);
  expect(ports.nativeShell).toBeInstanceOf(TauriNativeShell);
  expect(Object.values(ports).every((port) => port !== null)).toBe(true);
});

it('chooses the environment once, from the Tauri bridge on the window', async () => {
  const browser = await load();
  expect(browser.isNativeDesktop(window)).toBe(false);
  const first = browser.localPorts();
  expect(first.nativeShell).toBeNull();
  Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} });
  expect(browser.isNativeDesktop(window)).toBe(true);
  expect(browser.localPorts()).toBe(first);
  const desktop = await load();
  expect(desktop.localPorts().nativeShell).toBeInstanceOf(desktop.TauriNativeShell);
});
