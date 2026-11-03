#!/usr/bin/env node
// Internal packaging stage of `pnpm release:macos`. The release runs it as an owned child
// process of the signing wrapper so that an interruption reaches the synchronous payload
// and PKG commands (spawnSync) and the wrapper can still restore the keychain search list.
// Prints the package result as JSON, including notarization failures, and exits 1 on failure.
import { preparePayload } from './prepare-macos-release-payload.mjs';
import { buildPackages } from './build-macos-installer-pkgs.mjs';
import { notarizationArguments } from './macos-notarization.mjs';

try {
  const { payload: payloadOptions, packages } = JSON.parse(process.argv[2] ?? '');
  notarizationArguments(packages.notarization, packages.skipNotarization);
  preparePayload(payloadOptions);
  console.log(JSON.stringify(buildPackages(packages)));
} catch (error) {
  if (error.result) console.log(JSON.stringify(error.result));
  console.error(error.message);
  process.exitCode = 1;
}
