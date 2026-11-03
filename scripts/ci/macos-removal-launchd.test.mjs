// Registers temporary user launchd jobs for real XPC lifetime fixtures, so it changes
// system state while it runs. The macOS CI plan runs it (scripts/lab/native-check.mjs);
// local `pnpm test:scripts` does not.
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, copyFileSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

test('signed XPC reply and acknowledgement survive installed executable deletion with a retained listener', {skip:process.platform!=='darwin'}, async t=>{
  const dir=mkdtempSync(join(tmpdir(),'mixrelay-removal-reply-'));t.after(()=>rmSync(dir,{recursive:true,force:true}));
  const binary=join(dir,'fixture');
  function run(program,args,timeout=30000){return spawnSync(program,args,{encoding:'utf8',timeout});}
  const identity=process.env.MIXRELAY_LIFETIME_TEST_IDENTITY ?? '-';
  const team=identity.match(/^Developer ID Application: [^\r\n]+ \(([A-Z0-9]{10})\)$/)?.[1];
  assert.ok(identity==='-' || team,'fixture uses an explicit valid publisher');
  const requirements=team ? [`-DMR_FIXTURE_REQUIREMENT=${JSON.stringify(`anchor apple generic and identifier "io.mixrelay.removal.fixture" and certificate leaf[subject.OU] = "${team}" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists`)}`] : [];
  const compiled=run('/usr/bin/clang',['-fobjc-arc','-fblocks','-Werror',...requirements,'native/macos/installer/tests/removal_reply_lifetime.m','-framework','Foundation','-o',binary]);
  assert.equal(compiled.status,0,compiled.stderr);
  const signed=run('/usr/bin/codesign',['--force','--sign',identity,'--options','runtime',...(team?['--timestamp']:[]),'--identifier','io.mixrelay.removal.fixture',binary]);
  assert.equal(signed.status,0,signed.stderr);
  for(const mode of ['installed','server-only','handoff']){
    const installed=join(dir,`installed-${mode}`),stable=join(dir,`private-${mode}`);
    const client=join(dir,`client-${mode}`),privateClient=join(dir,`private-client-${mode}`);
    copyFileSync(binary,installed);copyFileSync(binary,stable);
    copyFileSync(binary,client);copyFileSync(binary,privateClient);
    const label=`io.mixrelay.removal.fixture.${process.pid}.${mode}`;
    const plist=join(dir,`${mode}.plist`);
    writeFileSync(plist,JSON.stringify({Label:label,ProgramArguments:[installed,'server',label,installed,stable,mode==='installed'?'installed':'handoff',client],MachServices:{[label]:true}}));
    const converted=run('/usr/bin/plutil',['-convert','xml1',plist]);assert.equal(converted.status,0,converted.stderr);
    const domain=`gui/${process.getuid()}`;
    const registered=run('/bin/launchctl',['bootstrap',domain,plist]);assert.equal(registered.status,0,registered.stderr);
    try{
      const result=run(client,['client',label,client,privateClient,mode==='handoff'?'handoff':'installed']);
      assert.equal(result.error,undefined);
      assert.equal(result.status,mode==='installed'?1:0,result.stdout+result.stderr);
      assert.match(result.stdout,mode==='installed'?/deleted sender rejected/:/signed success reply accepted/);
      assert.equal(existsSync(installed),false,'fixture transaction removed installed executable');
      assert.equal(existsSync(client),false,'fixture transaction removed installed UI executable');
      const deadline=Date.now()+1500;
      if(mode!=='installed'){
        while(!existsSync(stable+'.ack') && Date.now()<deadline) await new Promise(resolve=>setTimeout(resolve,20));
        assert.equal(existsSync(stable+'.ack'),true,'signed UI ACK arrived without a timeout fallback');
        assert.equal(existsSync(stable),false,'helper private copy retired after ACK');
      }else{
        await new Promise(resolve=>setTimeout(resolve,200));
        assert.equal(existsSync(stable+'.ack'),false,'rejected reply cannot be acknowledged');
      }
    }finally{run('/bin/launchctl',['bootout',`${domain}/${label}`]);}
  }
});
test('real idle listener cleans its private executable when its temporary user job is booted out', {skip:process.platform!=='darwin'}, async t=>{
  const dir=mkdtempSync(join(tmpdir(),'mixrelay-removal-idle-'));t.after(()=>rmSync(dir,{recursive:true,force:true}));
  const binary=join(dir,'private-listener'),evidence=join(dir,'cleanup'),ready=join(dir,'ready');
  function run(program,args){return spawnSync(program,args,{encoding:'utf8',timeout:10000});}
  const compiled=run('/usr/bin/clang',['-fobjc-arc','-fblocks','-Werror','native/macos/installer/tests/removal_idle_lifetime.m','-framework','Foundation','-framework','Security','-o',binary]);
  assert.equal(compiled.status,0,compiled.stderr);
  const label=`io.mixrelay.removal.idle.fixture.${process.pid}`,domain=`gui/${process.getuid()}`,plist=join(dir,'job.plist');
  writeFileSync(plist,JSON.stringify({Label:label,ProgramArguments:[binary,binary,evidence,ready],MachServices:{'io.mixrelay.driver.removal':true}}));
  assert.equal(run('/usr/bin/plutil',['-convert','xml1',plist]).status,0);
  const registered=run('/bin/launchctl',['bootstrap',domain,plist]);assert.equal(registered.status,0,registered.stderr);
  try{
    const started=run('/bin/launchctl',['kickstart',`${domain}/${label}`]);assert.equal(started.status,0,started.stderr);
    const deadline=Date.now()+3000;
    while(!existsSync(ready) && Date.now()<deadline) await new Promise(resolve=>setTimeout(resolve,20));
    assert.equal(existsSync(ready),true,'listener initialized');
    const stopped=run('/bin/launchctl',['bootout',`${domain}/${label}`]);assert.equal(stopped.status,0,stopped.stderr);
    while(!existsSync(evidence) && Date.now()<deadline) await new Promise(resolve=>setTimeout(resolve,20));
    assert.equal(existsSync(evidence),true,'SIGTERM invoked the production idle callback path');
    assert.equal(existsSync(binary),false,'private fixture executable cleaned');
  }finally{run('/bin/launchctl',['bootout',`${domain}/${label}`]);}
});
