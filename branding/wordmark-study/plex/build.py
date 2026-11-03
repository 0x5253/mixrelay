"""Draft lettering, derived from the unmodified Core Plex Medium font.

Run with fontTools, brotli, uharfbuzz and reportlab on PYTHONPATH.
Only the Illustrator master and PNG review sheets are user-facing artwork.
SVG/PDF intermediates live under /tmp, not the production identity directory.
"""
from pathlib import Path
from io import BytesIO
import copy, hashlib, html, json, subprocess
import uharfbuzz as hb
from fontTools.ttLib import TTFont
from fontTools.pens.recordingPen import RecordingPen, DecomposingRecordingPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.basePen import BasePen
from fontTools.svgLib.path import parse_path
from reportlab.pdfgen.canvas import Canvas

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
TEMP = Path('/tmp/mixrelay-plex-wordmark')
TEMP.mkdir(exist_ok=True)
FONTS = ROOT/'apps/control-ui/src/core/theme/fonts'
SYMBOL = json.loads((ROOT/'branding/identity/source/symbol.json').read_text())
core = json.loads(subprocess.check_output([
    'node','--experimental-strip-types','--input-type=module','-e',
    'import {brandColors,brandSizes} from "./apps/control-ui/src/core/theme/tokens.ts"; console.log(JSON.stringify({brandColors,brandSizes}));'
],cwd=ROOT,text=True))
colors = core['brandColors']
INK, PAPER, RED, DARK, INVERSE, MUTED = [colors[k] for k in ['ink','paper','primary','dark','inverse','muted']]
TEXT = 'MixRelay'
GAP_RATIO = .26
FONT_FILE = FONTS/'IBMPlexSans-Medium.woff2'
font = TTFont(FONT_FILE)
glyphs = font.getGlyphSet()
font.flavor = None
data = BytesIO(); font.save(data)
hbf = hb.Font(hb.Face(data.getvalue()))
cap = font['OS/2'].sCapHeight
font_cache = {FONT_FILE.name:font}
hb_cache = {id(font):hbf}

def shape(text, family=font):
    b = hb.Buffer(); b.add_str(text); b.guess_segment_properties()
    hf = hb_cache.get(id(family))
    if hf is None:
        family.flavor = None; d = BytesIO(); family.save(d)
        hf = hb.Font(hb.Face(d.getvalue()))
        hb_cache[id(family)] = hf
    hb.shape(hf, b)
    result = []; x = 0
    for info, pos in zip(b.glyph_infos, b.glyph_positions):
        result.append((family.getGlyphName(info.codepoint), x+pos.x_offset, pos.y_offset, pos.x_advance))
        x += pos.x_advance
    return result

original = {}
for ch in TEXT:
    pen = DecomposingRecordingPen(glyphs); glyphs[font.getBestCmap()[ord(ch)]].draw(pen)
    original[ch] = pen.value

def revised(ch, variant):
    ops = copy.deepcopy(original[ch])
    if variant == 'A': return ops
    # Optical correction in the artwork, never in the font file.
    if ch == 'i':
        if variant == 'C':
            return [('moveTo', ((78, 589),)), ('lineTo', ((187, 589),)),
                    ('lineTo', ((187, 698),)), ('lineTo', ((78, 698),)), ('closePath', ())] + ops[8:]
        # First contour only: lower the rounded Plex dot by 46 font units.
        ops = [(op, tuple((x, y-46 if index <= 6 else y) for x,y in pts)) for index,(op,pts) in enumerate(ops)]
    if ch == 'l':
        ops = [(op, tuple((245 if x == 259 else x, 698 if y == 740 else y) for x,y in pts)) for op,pts in ops]
        if variant == 'C':
            # Retain a foot for l/I distinction; straighten the lower junction.
            ops = [('moveTo', ((78,0),)), ('lineTo', ((78,698),)), ('lineTo', ((187,698),)),
                   ('lineTo', ((187,89),)), ('lineTo', ((245,89),)), ('lineTo', ((245,0),)), ('closePath', ())]
    return ops

ADJUST = {
 'A': {},
 'B': {'Mi':-12,'ix':-8,'xR':-30,'Re':-12,'el':-10,'la':-10,'ay':-12},
 'C': {'Mi':-12,'ix':-8,'xR':-30,'Re':-12,'el':-10,'la':-10,'ay':-12},
}

def records(variant):
    result=[]; shift=0
    for index,(name,x,y,advance) in enumerate(shape(TEXT)):
        ch = TEXT[index]
        result.append({'character':ch,'glyph':name,'x':x+shift,'y':y,'advance':advance,
                       'commands':revised(ch,variant)})
        if index < len(TEXT)-1: shift += ADJUST[variant].get(TEXT[index:index+2],0)
    return result

def replay(ops, pen):
    for method,args in ops: getattr(pen,method)(*args)

class CanvasPen(BasePen):
    def __init__(self, canvas):
        super().__init__(None)
        self.path=canvas.beginPath()
    def _moveTo(self,p): self.path.moveTo(*p)
    def _lineTo(self,p): self.path.lineTo(*p)
    def _curveToOne(self,p1,p2,p3): self.path.curveTo(*p1,*p2,*p3)
    def _closePath(self): self.path.close()
    def _endPath(self): pass

def path(ops, transform):
    p=SVGPathPen(None); replay(ops, TransformPen(p,transform)); return p.getCommands()

class Sheet:
    def __init__(self,name,width=1100,height=760,bg=PAPER):
        self.name,self.width,self.height,self.bg=name,width,height,bg
        self.elements=[]
        self.pdf=Canvas(str(TEMP/(name+'.pdf')),pagesize=(width,height))
        self.pdf.translate(0,height); self.pdf.scale(1,-1)
        self.rect(0,0,width,height,bg,'paper')
    def draw(self,ops,transform,color,name):
        if not ops: return
        d=path(ops,transform)
        self.elements.append('<path id="'+html.escape(name,quote=True)+'" fill="'+color+'" d="'+d+'"/>')
        p=CanvasPen(self.pdf); replay(ops,TransformPen(p,transform))
        self.pdf.setFillColor(color); self.pdf.drawPath(p.path,fill=1,stroke=0)
    def rect(self,x,y,w,h,color,name='rectangle'):
        self.draw([('moveTo',((x,y),)),('lineTo',((x+w,y),)),('lineTo',((x+w,y+h),)),('lineTo',((x,y+h),)),('closePath',())],(1,0,0,1,0,0),color,name)
    def label(self,text,x,y,size=13,color=MUTED,mono=False,medium=False):
        filename='IBMPlexMono-Regular.woff2' if mono else 'IBMPlexSans-Medium.woff2' if medium else 'IBMPlexSans-Regular.woff2'
        if filename not in font_cache: font_cache[filename]=TTFont(FONTS/filename)
        f=font_cache[filename]
        gs=f.getGlyphSet(); sc=size/1000
        for index,(g,dx,dy,advance) in enumerate(shape(text,f)):
            p=DecomposingRecordingPen(gs); gs[g].draw(p)
            self.draw(p.value,(sc,0,0,-sc,x+dx*sc,y),color,'note-'+str(len(self.elements))+'-'+str(index))
    def word(self,variant,x,baseline,capheight,color=INK,name=None):
        sc=capheight/cap
        self.elements.append('<g id="wordmark-'+variant+'-'+str(len(self.elements))+'">')
        for i,r in enumerate(records(variant)):
            self.draw(r['commands'],(sc,0,0,-sc,x+(r['x']-86)*sc,baseline),color,(name or variant)+'-'+str(i)+'-'+r['character'])
        self.elements.append('</g>')
        last=records(variant)[-1]
        return (last['x']+502-86)*sc
    def symbol(self,x,y,height,color=RED,name='Optical-Grid'):
        p=RecordingPen(); parse_path(SYMBOL['path'],p)
        sc=height/76; self.draw(p.value,(sc,0,0,sc,x-10*sc,y-10*sc),color,name)
    def lockup(self,variant,x,y,symbolheight,capheight,gap,color=INK,symbolcolor=RED,alignment='baseline'):
        self.symbol(x,y,symbolheight,symbolcolor)
        # M/R/l define the height and baseline; y is a descender, not an alignment edge.
        baseline=y+symbolheight
        if alignment=='previous-ink-center':
            sc=capheight/cap
            top=cap + (46 if variant=='A' else 0)
            baseline=y+symbolheight/2+(top-200)*sc/2
        return self.word(variant,x+symbolheight+gap,baseline,capheight,color)
    def finish(self):
        self.pdf.showPage(); self.pdf.save()
        body=''.join(self.elements)
        svg=f'<svg xmlns="http://www.w3.org/2000/svg" width="{self.width}" height="{self.height}" viewBox="0 0 {self.width} {self.height}"><g id="{self.name}">{body}</g></svg>'
        (TEMP/(self.name+'.svg')).write_text(svg)
        return body

def main():
    sheets=[]
    s=Sheet('01-Optical-colors')
    s.label('MIXRELAY / OPTICAL',48,42,14,INK,True)
    s.label('SELECTED DRAFT / CAP 1.00H / SHARED BASELINE / GAP 0.26H',48,65,11,MUTED,True)
    s.lockup('B',48,112,96,96,96*GAP_RATIO)
    s.label('PRIMARY / brand.primary + brand.ink',48,253,11,MUTED,True)
    s.lockup('B',48,302,96,96,96*GAP_RATIO,INK,INK)
    s.label('MONOCHROME / brand.ink',48,443,11,MUTED,True)
    s.rect(48,482,1004,220,DARK)
    s.lockup('B',80,520,88,88,88*GAP_RATIO,INVERSE,INVERSE)
    s.label('INVERSE / brand.inverse',80,672,11,INVERSE,True)
    sheets.append(s)

    s=Sheet('02-Optical-construction')
    s.label('OPTICAL / CONSTRUCTION',48,42,14,INK,True)
    s.label('Cap = symbol height. Baseline = symbol bottom. Gap = 0.26H.',48,67,15)
    s.lockup('B',48,134,92,92,92*GAP_RATIO)
    for yy in [134,226-520/698*92,226]: s.rect(48,yy,778,.5,'#b6b9ab')
    s.label('CAP / 698',865,125,10,MUTED,True)
    s.label('X-HEIGHT / 520',865,166,10,MUTED,True)
    s.label('BASELINE',865,249,10,MUTED,True)
    s.label('01  PAIR SPACING',48,294,13,INK,True)
    s.label('Mi     ix     xR     Re     el     la     ay',48,325,17,INK,True)
    s.label('-12    -8     -30    -12    -10    -10    -12',48,352,17,MUTED,True)
    s.label('Additional font units after OpenType kerning; UPM 1000.',48,379,12)
    for j,ch in enumerate(['i','l']):
        x=48+j*270
        s.label('02  DOT HEIGHT' if ch=='i' else '03  L HEIGHT / FOOT',x,436,12,INK,True)
        s.draw(revised(ch,'B'),(.18,0,0,-.18,x+35,609),INK,'optical-'+ch)
        s.label('Optical / cap aligned',x,640,11)
    s.label('04  PRESERVE',640,436,12,INK,True)
    s.label('M structure / R bowl and leg',640,470,16,INK)
    s.label('Open e / two-storey a / full y tail',640,501,16,INK)
    s.label('y descends 0.287H below baseline.',640,538,14)
    s.label('Keep the symbol, proportions and glyph shapes. Scale uniformly.',48,718,13)
    sheets.append(s)

    s=Sheet('03-Optical-scale-and-use')
    s.label('OPTICAL / SCALE AND USE',48,42,14,INK,True)
    s.label('Same lettering and 0.26H gap throughout. Static specimens.',48,67,13)
    for x,canvas in [(48,32),(406,24),(766,16)]:
        h=canvas*76/96
        s.lockup('B',x,111,h,h,h*GAP_RATIO)
        s.label('CANVAS '+str(canvas)+' / CAP '+format(h,'.2f'),x,172,10,MUTED,True)
    s.rect(48,208,1004,88,DARK)
    for x,canvas in [(80,32),(438,24),(798,16)]:
        h=canvas*76/96
        s.lockup('B',x,236,h,h,h*GAP_RATIO,INVERSE,INVERSE)
    s.rect(48,336,1004,75,INVERSE)
    s.lockup('B',72,361,19,19,19*GAP_RATIO)
    s.label('Documentation',648,381,13,INK)
    s.label('Download',826,381,13,INK)
    s.label('EN',984,381,12,INK,True)
    s.label('Static document header / canvas 24',48,447,12)
    s.label('WORDMARK WITHOUT SYMBOL',48,508,12,MUTED,True)
    s.word('B',48,623,65)
    s.label('Preserve room below the baseline for y. Minimum size awaits installed-app review.',48,710,12)
    sheets.append(s)

    bodies=[s.finish() for s in sheets]
    placements=[(0,0),(1180,0),(0,840)]
    combined=''.join(f'<g id="{s.name}" transform="translate({x} {y})">{b}</g>' for s,b,(x,y) in zip(sheets,bodies,placements))
    (TEMP/'mixrelay-plex-wordmark-review.svg').write_text(f'<svg xmlns="http://www.w3.org/2000/svg" width="2280" height="1600" viewBox="0 0 2280 1600">{combined}</svg>')
    payload={'status':'draft','selectedVariant':'B / Optical','date':'2026-10-12','text':TEXT,'fontSource':str(FONT_FILE.relative_to(ROOT)),
             'fontSha256':hashlib.sha256(FONT_FILE.read_bytes()).hexdigest(),'fontUPM':1000,'capHeight':cap,
             'variants':{'B':{'pairAdjustments':ADJUST['B'],'glyphs':records('B')}},
             'symbolSource':'branding/identity/source/symbol.json','proposedCapToSymbol':1.0,'proposedGapToSymbol':GAP_RATIO,
             'alignment':{'top':'M/R/l cap = symbol silhouette top','baseline':'M/R/l baseline = symbol silhouette bottom','descender':200/698,'curveOvershoot':12/698},
             'colorSource':'apps/control-ui/src/core/theme/tokens.ts','colors':colors,'existingSymbolCanvasSizes':core['brandSizes'],
             'artboards':[{'name':s.name,'x':x,'y':y,'width':s.width,'height':s.height} for s,(x,y) in zip(sheets,placements)]}
    (HERE/'source.json').write_text(json.dumps(payload,indent=2)+'\n')
    print(json.dumps({'intermediates':str(TEMP),'artboards':payload['artboards']}))

if __name__=='__main__': main()
