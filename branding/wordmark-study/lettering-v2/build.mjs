import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { brandColors as c } from '../../../apps/control-ui/src/core/theme/tokens.ts';

const root = path.dirname(fileURLToPath(import.meta.url));
const read = p => JSON.parse(fs.readFileSync(path.join(root, p), 'utf8'));
const source = read('source.json');
const symbol = read('../../identity/source/symbol.json');
const original = read('../reference-current.json');
const sonolog = read('sonolog-reference.json');
const n = v => +v.toFixed(5);
const esc = v => v.replaceAll('&', '&amp;').replaceAll('<', '&lt;');
const svg = (body, w, h, title) => `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" role="img"><title>${esc(title)}</title>${body}</svg>\n`;
const write = (file, data) => fs.writeFileSync(path.join(root, file), data);
const rect = (x,y,w,h,fill) => `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${fill}"/>`;
const txt = (x,y,text,size=14,color=c.muted,weight=400) => `<text x="${x}" y="${y}" font-family="Helvetica Neue,Arial,sans-serif" font-size="${size}" font-weight="${weight}" fill="${color}">${esc(text)}</text>`;
const rule = y => `<path d="M64 ${y}H1136" stroke="${c.muted}" stroke-opacity=".22"/>`;
const mark = (x,y,h,color,outline=false) => `<path d="${symbol.path}" transform="translate(${n(x-10*h/76)} ${n(y-10*h/76)}) scale(${n(h/76)})" ${outline?`fill="none" stroke="${color}" stroke-width="2" stroke-linejoin="round"`:`fill="${color}"`}/>`;
const models = source.variants.map(variant => {
  let cursor=0;
  const glyphs = [...source.text].map((ch,i) => {
    const glyph = variant.glyphs[ch] ?? source.glyphs[ch];
    const item = {...glyph, character:ch, x:cursor};
    cursor += glyph.width + (variant.gaps[i] ?? 0);
    return item;
  });
  return {...variant,glyphs,width:cursor};
});
const lettering = (model,x,y,cap,color) => `<g transform="translate(${n(x)} ${n(y)}) scale(${n(cap/100)})" fill="${color}" fill-rule="evenodd">${model.glyphs.map(g=>`<path data-letter="${g.character}" d="${g.path}" transform="translate(${g.x} 0)"/>`).join('')}</g>`;
const lockup = (model,x,y,h,color,typeColor=color,outline=false,ratio=.75) => mark(x,y,h,color,outline)+lettering(model,x+1.4*h,y+(.86-ratio)*h,ratio*h,typeColor);
const oldWord = (x,y,cap,color) => `<path d="${original.path}" transform="translate(${n(x-original.bounds[0]*cap/original.capHeight)} ${n(y+cap)}) scale(${n(cap/original.capHeight)})" fill="${color}"/>`;
const sonoWord = (x,y,cap,color) => {
  const scale=cap/(sonolog.bounds[3]-sonolog.bounds[1]);
  return `<path d="${sonolog.path}" transform="translate(${n(x-sonolog.bounds[0]*scale)} ${n(y-sonolog.bounds[1]*scale)}) scale(${n(scale)})" fill="${color}"/>`;
};
for (const model of models) {
  const width=Math.ceil(32+64*1.4+model.width*.48);
  write(`${model.id}-wordmark.svg`, svg(lettering(model,16,16,64,c.ink),Math.ceil(model.width*.64+32),114,`MixRelay ${model.name} original lettering`));
  for (const [tone, ink, typeInk, bg, outline] of [
    ['primary',c.primary,c.ink,null,false],['mono',c.ink,c.ink,null,false],
    ['inverse',c.inverse,c.inverse,c.dark,false],['outline',c.primary,c.ink,null,true],
  ]) write(`${model.id}-${tone}.svg`,svg((bg?rect(0,0,width,96,bg):'')+lockup(model,16,16,64,ink,typeInk,outline),width,96,`MixRelay ${model.name} ${tone} study`));
}

let board=rect(0,0,1200,1112,c.paper);
board+=txt(64,58,'MIXRELAY / LETTERING 02',14,c.muted,500);
board+=txt(64,111,'Drawn as one family.',32,c.ink,500);
board+=txt(64,146,'Eight original vector letters. Same symbol, same colors, same cap height.',15);
for(let i=0;i<models.length;i++){
  const m=models[i], y=194+i*366;
  board+=rule(y)+txt(64,y+36,`${m.id.toUpperCase()} / ${m.name.toUpperCase()}`,15,c.ink,500);
  board+=txt(64,y+61,i===0?'Flatter R + e shoulders / short l foot / returning y':'Fuller R + e bowls / straight l / direct diagonal y',13);
  board+=lockup(m,64,y+107,104,c.primary,c.ink);
  board+=txt(64,y+263,'MONOCHROME',11);
  board+=lettering(m,64,y+283,38,c.ink);
  board+=rect(856,y+24,280,304,c.dark)+txt(880,y+55,'INVERSE',11,c.inverse);
  board+=lockup(m,880,y+83,34,c.inverse);
  board+=txt(880,y+165,'24 PX SYMBOL / 18 PX TYPE',10,c.inverse);
  board+=lockup(m,880,y+181,24,c.inverse);
  board+=txt(880,y+251,'16 PX SYMBOL / 10 PX TYPE',10,c.inverse);
  board+=mark(880,y+268,16,c.inverse)+lettering(m,904,y+271,10,c.inverse);
}
board+=rule(944)+txt(64,980,'CURRENT / REFERENCE ONLY',11);
board+=oldWord(64,1003,25,c.muted);
board+=txt(440,980,'DESIGN CHANGES',11);
board+=txt(440,1008,'M width + valley / R bowl + leg / e aperture',14,c.ink);
board+=txt(440,1031,'Square i dot / x intersection / l, a, y terminals',14,c.ink);
board+=txt(64,1080,'Lettering study; selected symbol and production assets retained.',12);
write('comparison.svg',svg(board,1200,1112,'MixRelay custom lettering A and B comparison'));

// Same physical cap height for the reference and the new lettering; no width fitting.
let detail=rect(0,0,1200,1232,c.paper);
detail+=txt(64,58,'FORM / BEFORE AND AFTER',15,c.muted,500);
detail+=txt(64,108,'The letters that changed.',30,c.ink,500);
detail+=txt(64,142,'Reference = current Helvetica Neue Regular / A and B = original contours',14);
const columns=['M','R','e','y'];
for(let i=0;i<columns.length;i++){
  const ch=columns[i],x=64+i*276;
  detail+=txt(x,210,ch,18,c.ink,500);
  const originalGlyph=original.glyphs.find(g=>g.character===ch);
  const scale=96/original.capHeight;
  const clipId=`old-${ch}`;
  detail+=`<defs><clipPath id="${clipId}" clipPathUnits="userSpaceOnUse"><rect x="${x}" y="248" width="${n((originalGlyph.bounds[2]-originalGlyph.bounds[0])*scale)}" height="130"/></clipPath></defs>`;
  detail+=txt(x,239,'CURRENT',10);
  detail+=`<g clip-path="url(#${clipId})"><path d="${original.path}" transform="translate(${n(x-originalGlyph.bounds[0]*scale)} 344) scale(${n(scale)})" fill="${c.muted}"/></g>`;
  for(let j=0;j<models.length;j++){
    const g=models[j].glyphs.find(g=>g.character===ch),y=420+j*172;
    detail+=txt(x,y-17,models[j].id.toUpperCase(),11,c.ink,500);
    detail+=`<path d="${g.path}" transform="translate(${x} ${y}) scale(.96)" fill="${j===0?c.primary:c.ink}" fill-rule="evenodd"/>`;
    detail+=`<path d="M${x-4} ${y}H${x+108}M${x-4} ${y+96}H${x+108}" stroke="${c.muted}" stroke-opacity=".2"/>`;
  }
  const descriptions=[['M: 100% cap / 80% width','Open valley; straight stems'],['R: continuous bowl + straight leg','Two shoulders, same structure'],['e: 13-unit open aperture','10.5-unit lighter crossbar'],['y: 28-unit descender','Return or direct diagonal']];
  detail+=txt(x,759,descriptions[i][0],11,c.ink)+txt(x,782,descriptions[i][1],11);
}
detail+=rule(827)+txt(64,866,'SPACING / ALL PAIRS ARE SET INDIVIDUALLY',13,c.ink,500);
detail+=txt(64,897,'Mi   ix   xR   Re   el   la   ay',21,c.ink);
detail+=txt(64,929,`A:   ${models[0].gaps.join(' / ')}`,14);
detail+=txt(64,955,`B:   ${models[1].gaps.join(' / ')}`,14);
detail+=txt(64,984,'Values refer to ink-box gaps in 100-unit cap geometry; optical review remains visual.',12);
detail+=rule(1018)+txt(64,1055,'SECONDARY LETTERS / A',11);
const secondary=['i','x','l','a'];
for(let i=0;i<secondary.length;i++){
 const g=models[0].glyphs.find(g=>g.character===secondary[i]),x=64+i*116;
 detail+=`<path d="${g.path}" transform="translate(${x} 1080) scale(.9)" fill="${c.ink}" fill-rule="evenodd"/>`;
}
detail+=txt(614,1116,'75-unit x-height / 12.5-unit stems',14,c.ink)+txt(614,1143,'No font outline is used in either candidate.',14,c.ink);
detail+=txt(64,1200,'This is lettering for one product name, not a full custom typeface.',12);
write('details.svg',svg(detail,1200,1232,'MixRelay original glyph design before and after'));

let context=rect(0,0,1200,1352,c.paper);
context+=txt(64,58,'COMPOSITION / FAMILY AND SCALE',15,c.muted,500);
context+=txt(64,110,'One product family. Actual sizes.',30,c.ink,500);
context+=txt(64,144,'Sonolog uses its existing outlined exploration; its identity is not changed.',14);
for(let i=0;i<models.length;i++){
 const m=models[i],y=202+i*202;
 context+=rule(y)+txt(64,y+33,m.id.toUpperCase(),14,c.ink,500);
 context+=sonoWord(64,y+77,36,c.primary);
 context+=lettering(m,624,y+77,36,c.primary);
 context+=txt(64,y+160,'SONOLOG / EXISTING REFERENCE',11)+txt(624,y+160,'MIXRELAY / NEW LETTERING',11);
}
context+=rule(630)+txt(64,669,'SYMBOL TO TYPE / A',13,c.ink,500);
for(let i=0;i<3;i++){
 const ratio=[.65,.75,.85][i],y=710+i*92;
 context+=txt(64,y+24,`${n(ratio*100)}% CAP`,11);
 context+=lockup(models[0],224,y,52,c.primary,c.ink,false,ratio);
}
context+=rule(1006)+txt(64,1043,'NATIVE 1X / COMPACT AND STANDARD',13,c.ink,500);
for(let i=0;i<models.length;i++){
 const m=models[i],x=64+i*540;
 context+=txt(x,1078,m.id.toUpperCase(),11,c.ink);
 context+=mark(x,1098,16,c.primary)+lettering(m,x+24,1101,10,c.ink);
 context+=lockup(m,x,1154,24,c.primary,c.ink);
 context+=rect(x,1210,480,80,c.dark)+mark(x+16,1226,16,c.inverse)+lettering(m,x+40,1229,10,c.inverse);
 context+=lockup(m,x+240,1226,24,c.inverse);
}
context+=txt(64,1321,'A 75% composition is proposed for display; compact preserves 16px symbol / 10px type.',12);
write('context.svg',svg(context,1200,1352,'MixRelay Sonolog family and native size review'));
write('review.svg',svg(board+`<g transform="translate(0 1184)">${detail}</g><g transform="translate(0 2496)">${context}</g>`,1200,3848,'MixRelay lettering review: comparison, glyph details and product family'));
write('manifest.json',JSON.stringify({status:source.status,text:source.text,origin:source.origin,
  selectedSymbol:symbol.name,source:'source.json',lockup:source.lockup,
  colors:c,variants:models.map(m=>({id:m.id,name:m.name,width:m.width,
    capHeight:100,xHeight:75,gaps:m.gaps,glyphs:m.glyphs.map(g=>({character:g.character,x:g.x,width:g.width,change:g.change}))})),
  productionUpdated:false},null,2)+'\n');
console.log('Generated two original lettering candidates, ten vector assets and three review sheets.');
