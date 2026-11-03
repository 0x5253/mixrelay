import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { distribution } from './build-macos-installer-pkgs.mjs';

test('native Installer has one fixed installation operation and no removal or program-execution gate', {skip:process.platform!=='darwin'},t=>{
 const root=mkdtempSync(join(tmpdir(),'mixrelay-install-only-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 function run(cmd,args){const r=spawnSync(cmd,args,{encoding:'utf8',timeout:30000});assert.equal(r.error,undefined);assert.equal(r.status,0,r.stderr);return r.stdout;}
 const xml=distribution('install.component.pkg','1.2.3');
 assert.doesNotMatch(xml,/system\.run|<script|remove|deleteSettings|InstallerPlugins/);
 assert.match(xml,/customize="never" allow-external-scripts="false"/);
 // The authorized transaction must observe and close the Driver manager itself
 // so that a previously running manager can be reopened after the update.
 assert.doesNotMatch(xml, /<app id="io\.mixrelay\.driver\.desktop"/);
 assert.doesNotMatch(xml, /<app id="io\.mixrelay\.desktop"/);
 run('/usr/bin/pkgbuild',['--nopayload','--identifier','io.mixrelay.installer.component','--version','1.2.3',join(root,'install.component.pkg')]);
 const resources=join(root,'Resources');mkdirSync(join(resources,'en.lproj'),{recursive:true});
 for(const name of ['INSTALLER-README.rtf'])writeFileSync(join(resources,'en.lproj',name),'{\\rtf1 Fixture notice}');
 const source=join(root,'Distribution');writeFileSync(source,xml);const pkg=join(root,'fixture.pkg');run('/usr/bin/productbuild',['--distribution',source,'--package-path',root,'--resources',resources,pkg]);
 const output=join(root,'choices.plist');writeFileSync(output,run('/usr/sbin/installer',['-pkg',pkg,'-target','/','-showChoicesXML']));
 const tree=JSON.parse(run('/usr/bin/plutil',['-convert','json','-o','-',output]));
 const choices=tree.flatMap(x=>x.childItems??[]);
 assert.equal(choices.length,1);assert.equal(choices[0].choiceIdentifier,'install');assert.equal(choices[0].choiceIsSelected,1);assert.equal(choices[0].choiceIsVisible,false);
});
