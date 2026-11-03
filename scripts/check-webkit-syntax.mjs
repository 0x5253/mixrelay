#!/usr/bin/env node
// Parses built JavaScript with WebKit's JavaScriptCore, the engine of the macOS app's
// WKWebView. Its parser rejects some constructs that other engines and the bundler's own
// checks accept (for example a minified `for(` header whose nested arrow body uses `in`);
// such a bundle never runs and the app window stays blank.
//   node scripts/check-webkit-syntax.mjs apps/control-ui/dist
// Skipped where JavaScriptCore's shell is unavailable (non-macOS hosts).
import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const JSC = '/System/Library/Frameworks/JavaScriptCore.framework/Versions/A/Helpers/jsc';

// `arguments` holds the paths passed after `--`; each is parsed as a module, never run.
const DRIVER =
  'for (const file of arguments) { try { checkModuleSyntax(read(file)); print("ok " + file); }' +
  ' catch (error) { print("error " + file + ": " + error); } }';

function scripts(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return scripts(path);
    return /\.(m?js)$/.test(entry.name) ? [path] : [];
  });
}

/** Returns one message per file JavaScriptCore cannot parse, or `null` when unavailable. */
export function webkitSyntaxErrors(directory, jsc = JSC) {
  if (!existsSync(jsc)) return null;
  const files = scripts(resolve(directory));
  if (files.length === 0) return [];
  const output = execFileSync(jsc, ['-e', DRIVER, '--', ...files], { encoding: 'utf8' });
  return output
    .split('\n')
    .filter((line) => line.startsWith('error '))
    .map((line) => line.slice('error '.length));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const directory = process.argv[2];
  if (!directory) {
    console.error('usage: node scripts/check-webkit-syntax.mjs DIRECTORY');
    process.exit(2);
  }
  const errors = webkitSyntaxErrors(directory);
  if (errors === null) {
    console.log('WebKit syntax check skipped: JavaScriptCore is unavailable on this host');
  } else if (errors.length > 0) {
    for (const error of errors) console.error(`WebKit cannot parse ${error}`);
    process.exit(1);
  } else {
    console.log(`WebKit parses every script in ${directory}`);
  }
}
