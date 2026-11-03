import type { ThemePreference } from '../domain/appearance';

/** Where the chosen appearance is kept between sessions. Implementations never throw: an
 * unavailable store reads the default and forgets writes. */
export interface ThemePreferenceStore {
  read(): ThemePreference;
  write(preference: ThemePreference): void;
  /** Reports a preference saved elsewhere, such as another window of the same app. */
  subscribe(listener: (preference: ThemePreference) => void): () => void;
}
