import sys, os, glob
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer
src, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)
for path in sorted(glob.glob(os.path.join(src, '*', '*.ttf'))):
    base = os.path.basename(path).split('[')[0]
    probe = TTFont(path)
    fam = probe['name'].getDebugName(16) or probe['name'].getDebugName(1)
    for w, style in ((400, 'Regular'), (600, 'SemiBold'), (700, 'Bold')):
        f = TTFont(path)
        axes = {a.axisTag: a.defaultValue for a in f['fvar'].axes}
        axes['wght'] = w
        inst = instancer.instantiateVariableFont(f, axes, updateFontNames=False)
        inst['OS/2'].usWeightClass = w
        for t in ('STAT',):
            if t in inst: del inst[t]
        name = inst['name']
        legacy_fam = fam if style != 'SemiBold' else f'{fam} SemiBold'
        legacy_sub = 'Bold' if style == 'Bold' else 'Regular'
        ps = f"{fam.replace(' ', '')}-{style}"
        for rec in list(name.names):
            if rec.nameID in (1, 2, 3, 4, 6, 16, 17, 25):
                name.removeNames(nameID=rec.nameID)
        for nid, val in ((1, legacy_fam), (2, legacy_sub), (3, ps), (4, f'{fam} {style}'), (6, ps), (16, fam), (17, style)):
            name.setName(val, nid, 3, 1, 0x409)
            name.setName(val, nid, 1, 0, 0)
        os2 = inst['OS/2']
        os2.fsSelection = (os2.fsSelection & ~0b1100001) | (0b100000 if style == 'Bold' else 0b1000000)
        inst['head'].macStyle = 1 if style == 'Bold' else 0
        dst = os.path.join(out, f'{base}-{style}.ttf')
        inst.save(dst)
        print(os.path.basename(dst), os.path.getsize(dst), fam)
