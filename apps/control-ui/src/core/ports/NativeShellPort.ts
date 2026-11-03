import type { ThemePreference } from '../domain/appearance';

/** Resolves once the native listener is registered; the result removes it. */
export type NativeSubscription = Promise<() => void>;

/** The desktop shell around the editor: app identity, native menus and the window. A browser
 * has no shell, so consumers receive this port or null. */
export interface NativeShellPort {
  version(): Promise<string>;
  /** The application menu's About item. */
  onAbout(listener: () => void): NativeSubscription;
  /** View → Show/Hide Graph List. */
  onGraphListToggle(listener: () => void): NativeSubscription;
  /** Keeps the menu label naming what the command will do next. */
  setGraphListVisible(visible: boolean): Promise<void>;
  setMenuLocale(locale: string): Promise<void>;
  setWindowTheme(preference: ThemePreference): Promise<void>;
}
