import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const MINIMUM_MACOS = JSON.parse(readFileSync(new URL('../support/environments.json', import.meta.url), 'utf8')).minimums.macos;

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  console.log(MINIMUM_MACOS);
}
