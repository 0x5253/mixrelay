/** One signed MixRelay installer: editor, runtime, HAL and removal helper. */
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { buildPackages, main, parseArgs } from './build-macos-installer-pkgs.mjs';

export const buildAppInstaller = buildPackages;
export const parseAppInstallerArgs = parseArgs;

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  process.exitCode = main();
}
