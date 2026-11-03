import { useState } from 'react';
import styles from './AudioNodeIcon.module.css';

/** The discovered app or platform icon wins; built-in glyphs appear only when it is absent or fails to load. */
export function AudioNodeIcon({ icon, iconUrl }: { icon: string; iconUrl?: string }) {
  const [failedUrl, setFailedUrl] = useState<string>();
  if (iconUrl && iconUrl !== failedUrl) {
    return (
      <img alt="" className={styles.image} src={iconUrl} onError={() => setFailedUrl(iconUrl)} />
    );
  }
  const kind = icon.toUpperCase();
  if (kind === 'APP') {
    return (
      <svg aria-hidden="true" className={styles.glyph} viewBox="0 0 16 16">
        <rect height="11" width="12" x="2" y="2.5" />
        <path d="M2.5 5.5h11" />
        <circle cx="4.25" cy="4" r=".45" />
      </svg>
    );
  }
  if (kind === 'GAIN' || kind === 'MIX') {
    return (
      <svg aria-hidden="true" className={styles.glyph} viewBox="0 0 16 16">
        <path d="M3 3v10M8 3v10M13 3v10" />
        <path d="M1.5 6h3M6.5 10h3M11.5 7.5h3" />
      </svg>
    );
  }
  if (kind === 'ADC' || kind === 'MIC') {
    return (
      <svg aria-hidden="true" className={styles.glyph} viewBox="0 0 16 16">
        <rect height="8" width="5" x="5.5" y="2" />
        <path d="M3.5 7.5a4.5 4.5 0 0 0 9 0M8 12v2M5.5 14h5" />
      </svg>
    );
  }
  if (kind === 'DAC') {
    return (
      <svg aria-hidden="true" className={styles.glyph} viewBox="0 0 16 16">
        <path d="M2.5 6h3l3-2.5v9L5.5 10h-3zM11 6a3 3 0 0 1 0 4M12.5 4.5a5 5 0 0 1 0 7" />
      </svg>
    );
  }
  return (
    <svg aria-hidden="true" className={styles.glyph} viewBox="0 0 16 16">
      <rect height="10" width="12" x="2" y="3" />
      <circle cx="6" cy="8" r="1" />
      <circle cx="10" cy="8" r="1" />
    </svg>
  );
}
