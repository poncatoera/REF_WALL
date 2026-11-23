# Draws the Wall app icon: apps/desktop/assets/icon.png (window icon) and
# wall.ico (wall.exe and the installer). Needs Pillow.
#   python3 tools/make-icon.py apps/desktop/assets
import sys
from PIL import Image, ImageDraw, ImageFilter
S=1024
def card(img, box, fill, radius, angle, shadow=True):
    x0,y0,x1,y1=box; w=x1-x0; h=y1-y0
    pad=80
    layer=Image.new("RGBA",(w+2*pad,h+2*pad),(0,0,0,0))
    d=ImageDraw.Draw(layer)
    if shadow:
        sh=Image.new("RGBA",layer.size,(0,0,0,0))
        ImageDraw.Draw(sh).rounded_rectangle((pad+6,pad+22,pad+w+6,pad+h+22),radius,fill=(0,0,0,150))
        sh=sh.filter(ImageFilter.GaussianBlur(22))
        layer=Image.alpha_composite(layer,sh); d=ImageDraw.Draw(layer)
    d.rounded_rectangle((pad,pad,pad+w,pad+h),radius,fill=fill)
    return layer, (x0-pad, y0-pad), angle
def paste(img, spec, extra=None):
    layer,(px,py),angle=spec
    if extra: extra(ImageDraw.Draw(layer))
    r=layer.rotate(angle,resample=Image.BICUBIC,expand=False)
    img.alpha_composite(r,(px,py))
img=Image.new("RGBA",(S,S),(0,0,0,0))
# tile background: graphite with soft vertical gradient
bg=Image.new("RGBA",(S,S),(0,0,0,0))
grad=Image.new("RGBA",(S,S))
gd=ImageDraw.Draw(grad)
for y in range(S):
    t=y/S
    c=tuple(int(a+(b-a)*t) for a,b in zip((50,55,66),(22,24,30)))
    gd.line([(0,y),(S,y)],fill=c+(255,))
mask=Image.new("L",(S,S),0)
ImageDraw.Draw(mask).rounded_rectangle((40,40,S-40,S-40),210,fill=255)
img.paste(grad,(0,0),mask)
# hairline edge
ImageDraw.Draw(img).rounded_rectangle((40,40,S-40,S-40),210,outline=(255,255,255,40),width=6)
# back card: note (paper)
def note_lines(d):
    for i,yy in enumerate((150,215,280)):
        d.rounded_rectangle((80+48,80+yy-60,80+48+(250 if i<2 else 160),80+yy-60+26),13,fill=(190,186,178,255))
paste(img, card(img,(470,170,850,560),(243,241,236,255),46,-8), note_lines)
# vermilion small card
paste(img, card(img,(560,560,860,830),(217,71,47,255),40,6))
# front picture card: blue, with a "mountain" picture motif
def pic(d):
    p=80
    d.polygon([(p+40,p+330),(p+170,p+170),(p+260,p+270),(p+320,p+215),(p+420,p+330)],fill=(24,42,82,255))
    d.ellipse((p+300,p+70,p+380,p+150),fill=(236,244,255,255))
paste(img, card(img,(170,300,630,760),(110,168,255,255),46,4), pic)
img.save(sys.argv[1]+"/icon-1024.png")
sizes=[16,20,24,32,40,48,64,128,256]
img.resize((256,256),Image.LANCZOS).save(sys.argv[1]+"/icon.png")
img.save(sys.argv[1]+"/wall.ico",sizes=[(s,s) for s in sizes])
