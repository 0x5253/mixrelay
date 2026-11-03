import fs from 'node:fs';

// The Tauri CLI renders the same vector master before this step.
fs.copyFileSync(new URL('./identity/exports/1024x1024.png', import.meta.url), new URL('./icon.png', import.meta.url));
