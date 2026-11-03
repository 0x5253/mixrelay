import { defaultThemePreference, themePreference, themeStorageKey } from '../../domain/appearance';
import type { ThemePreferenceStore } from '../../ports/ThemePreferenceStore';

/** The page facilities the store uses. `storage` is a getter because reading
 * `localStorage` itself throws where site data is blocked. */
export interface ThemeStorageEnvironment {
  storage(): Pick<Storage, 'getItem' | 'setItem'>;
  events: Pick<Window, 'addEventListener' | 'removeEventListener'>;
}

export function themePreferenceStorage(environment: ThemeStorageEnvironment): ThemePreferenceStore {
  return {
    read() {
      try {
        return themePreference(environment.storage().getItem(themeStorageKey));
      } catch {
        return defaultThemePreference;
      }
    },
    write(preference) {
      try {
        environment.storage().setItem(themeStorageKey, preference);
      } catch {
        // A blocked store must not prevent changing this session's appearance.
      }
    },
    subscribe(listener) {
      // A null key means another window cleared all of this origin's storage.
      const changed = (event: StorageEvent) => {
        if (event.key === themeStorageKey || event.key === null)
          listener(themePreference(event.newValue));
      };
      environment.events.addEventListener('storage', changed);
      return () => environment.events.removeEventListener('storage', changed);
    },
  };
}

/** This page's local storage; created lazily so a store that never runs touches nothing. */
export function browserThemePreferenceStorage(): ThemePreferenceStore {
  return themePreferenceStorage({ storage: () => localStorage, events: window });
}
