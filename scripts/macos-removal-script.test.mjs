import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { removalLaunchdTemplate } from './prepare-macos-release-payload.mjs';

test('removal helper accepts only a validated publisher and fixed one-operation configuration',()=>{
  for(const team of ['bad','AB12C3D4E5; arbitrary','AB12C3D4E5\n']) assert.throws(()=>removalLaunchdTemplate(team));
  const plist=removalLaunchdTemplate('AB12C3D4E5');
  assert.match(plist,/<string>removal-helper<\/string>/);
  assert.doesNotMatch(plist,/<key>(?:RunAtLoad|KeepAlive|UserName)<\/key>/);
});

test('native XPC rejects malformed and unauthenticated removal requests without mutation', {skip:process.platform!=='darwin'},t=>{
  const dir=mkdtempSync(join(tmpdir(),'mixrelay-removal-protocol-')); t.after(()=>rmSync(dir,{recursive:true,force:true}));
  const binary=join(dir,'protocol');
  const compiled=spawnSync('/usr/bin/clang',['-fobjc-arc','-fblocks','-Werror','native/macos/installer/tests/removal_protocol.m','-framework','Foundation','-framework','Security','-o',binary],{encoding:'utf8',timeout:30000});
  assert.equal(compiled.status,0,compiled.stderr);
  const run=spawnSync(binary,[],{encoding:'utf8',timeout:10000});
  assert.equal(run.status,0,run.stderr); assert.match(run.stdout,/rejects malformed/);
});
