import { getVersion } from '@tauri-apps/api/app';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { ThemePreference } from '../../domain/appearance';
import type { NativeShellPort } from '../../ports/NativeShellPort';
import { string } from '../decode';
import { type NativeInvoke, type NativeListen, tauriInvoke, tauriListen } from './nativeInvoke';

/** The native calls the shell needs, injectable so the adapter runs without Tauri. */
export interface TauriShellApi {
  invoke: NativeInvoke;
  listen: NativeListen;
  version(): Promise<unknown>;
  /** Tauri follows the operating system for `null`. */
  setTheme(theme: 'light' | 'dark' | null): Promise<void>;
}

const tauriShellApi: TauriShellApi = {
  invoke: tauriInvoke,
  listen: tauriListen,
  version: getVersion,
  setTheme: (theme) => getCurrentWindow().setTheme(theme),
};

export class TauriNativeShell implements NativeShellPort {
  constructor(private readonly api: TauriShellApi = tauriShellApi) {}

  async version() {
    return string(await this.api.version(), 'app version');
  }

  onAbout(listener: () => void) {
    return this.api.listen('app-about', () => listener());
  }

  onGraphListToggle(listener: () => void) {
    return this.api.listen('graph-list-toggle', () => listener());
  }

  async setGraphListVisible(visible: boolean) {
    await this.api.invoke('set_graph_list_visible', { visible });
  }

  async setMenuLocale(locale: string) {
    await this.api.invoke('set_native_menu_locale', { locale });
  }

  setWindowTheme(preference: ThemePreference) {
    return this.api.setTheme(preference === 'system' ? null : preference);
  }
}
