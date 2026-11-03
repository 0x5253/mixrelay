import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { JSC, webkitSyntaxErrors } from './check-webkit-syntax.mjs';

test('an unavailable engine skips the check instead of passing or failing it', () => {
  assert.equal(webkitSyntaxErrors('.', '/nonexistent/jsc'), null);
});

test('JavaScriptCore rejects the minified loop header that blanked the app', { skip: !existsSync(JSC) }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mixrelay-webkit-'));
  try {
    await mkdir(join(directory, 'assets'));
    // Valid ECMAScript that V8 accepts; WebKit fails to parse `in` inside the arrow body.
    await writeFile(
      join(directory, 'assets', 'broken.js'),
      'let t={};for(t.onmessage=e=>{if(`ended`in e)return};0;);export{}\n',
    );
    await writeFile(join(directory, 'assets', 'fine.js'), 'export const a = `x` in {x: 1};\n');
    await writeFile(join(directory, 'notes.txt'), 'not a script');
    const errors = webkitSyntaxErrors(directory);
    assert.equal(errors.length, 1);
    assert.match(errors[0], /broken\.js: SyntaxError/);
    await mkdir(join(directory, 'empty'));
    assert.deepEqual(webkitSyntaxErrors(join(directory, 'empty')), []);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
