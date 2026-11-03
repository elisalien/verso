"""Builds the bundled MilkDrop preset families of Verso.

Inputs, produced by the steps in scripts/milkdrop/README.md:
  pool/            Butterchurn JSON presets that are also in Cream of the Crop
  pool-render/     one preview per preset (milk_render example, `<stem>-0.png`)
  families.json    code-similarity clusters (cluster.py)

The curation below was made by looking at contact sheets of every preset:
clusters are split where remixes drifted into different looks, and presets
that render black, blown out or static are dropped.

Usage: python -X utf8 curate.py <scratch dir> <repo presets/milkdrop dir>
"""

import json
import os
import re
import shutil
import sys

from PIL import Image

# Whole clusters dropped (black, blown out, glitchy or static here).
REJECT_CLUSTERS = {14, 18, 21, 22, 27, 32, 38, 39, 41, 52, 58, 66, 71, 81, 88, 93,
                   101, 104, 119, 125, 131, 138, 143, 144, 148, 157, 175, 177, 178, 179}
# Single presets dropped from kept clusters.
REJECT_FILES = {
    'ORB - Lazer Ride.json',
    'orb - toxic goo.json',
    'fiShbRaiN + flexi - witchcraft 2.0 - mash0000 - no one cares about mi, the note (major third).json',
    '$$$ Royal - Mashup (489).json',
    'martin - chain breaker.json',
    'martin - crystal palace.json',
    'Milk Artist At our Best - FED - SlowFast Ft AdamFX n Martin - HD CosmoFX.json',
}
# Clusters split into several families (member indices).
SPLITS = {
    3: [[0], [1, 2, 3], [4, 5]],
    31: [[0], [1]],
    65: [[0], [1]],
    111: [[0], [1], [2], [3]],
    135: [[0], [1]],
    140: [[0], [1]],
    154: [[0], [1]],
    156: [[0], [1, 2], [3, 4, 5, 6, 7]],
    169: [[0, 2], [1], [3, 4], [5]],
    176: [[0], [1]],
}
# Remixes of one preset that ended in different clusters, regrouped as one
# family: (name, category, files).
GLOWSTICKS_05 = 'Eo.S. - glowsticks v2 05 and proton lights (+Krash′s beat code) _Phat_'
GROUPS = [
    ('Reaction Diffusion', 'Reaction', [
        'Geiss - Reaction Diffusion 2.json',
        'Geiss - Reaction Diffusion (Puddle Mix)s a aa aa.json',
        'AdamFx 2 Geiss, Zylot and Flexi - Reaction Diffusion 3 (Overload Mix 2) EATIT4 hypno.json',
        'NeW Adam Master Mashup FX 2 Geiss and Zylot - Reaction Diffusion 3 (Overload Mix 2) '
        'AMAZING MASUP Various + Hurricane Nightmare.json',
        'NeW Adam Master Mashup FX 2 Geiss - Reaction Diffusion 34 + Swelling Spiral  + Liquid Fire  '
        '+ Geiss an46.json',
    ]),
    ('Airhandler', 'Reaction', [
        'Aderrasi - Airhandler (Principle of Sharing).json',
        'AdamFx 2 Aderrasi - Airhandler (Last Breath - Calm)Ilusional Discontent2.json',
    ]),
    ('Disco Mix', 'Dancer', [
        'martin - disco mix 4.json',
        'adam eatit fx 2 martin - disco mix, lodus, geiss, ludicrous speed,flexi, aderrasi n hexcollie.json',
        'Adam Eatit Mashup FX 2 martin - disco mix + Lodus + Geiss + Ludicrous speed + Aderrasi 2_1.json',
    ]),
    ('Glowsticks', 'Dancer', [
        'Eo.S. + Geiss - glowsticks v2 02 (Relief Mix).json',
        'Eo.S. - glowsticks v2 03 music.json',
        '_Eo.S. - glowsticks v2 02 - Geiss HPF.json',
        GLOWSTICKS_05 + 'remix02b.json',
        GLOWSTICKS_05 + 'remix03 madhatter_v2.json',
        GLOWSTICKS_05 + 'remix07 recursive demons.json',
    ]),
    ('Jelly', 'Reaction', [
        'Flexi + stahlregen - jelly space.json',
        'Flexi + stahlregen - jelly showoff parade.json',
        'Flexi - reality tunnel.json',
        'flexi + nitorami - beat explorer (cn bc jelly 4).json',
        'flexi + stahlregen - jelly strike.json',
        'flexi - color strike.json',
        'flexi - hyperspaceflight (bn cn Jelly 4).json',
        'flexi - jelly fish mandala.json',
        'flexi, stahlregen, geiss + tobias wolfboi - space gelatine burst - mash0000 - '
        'chromatidal pool mirror blasphemy.json',
    ]),
    ('Sawtooth Grin', 'Drawing', ['sawtooth grin roam.json', 'sawtooth grin nz+ m10 w4.json']),
    ('Ed Geining Hateops', 'Waveform', [
        "suksma - ed geining hateops - flx i want you, i'm sick with you.json",
        'suksma - ed geining hateops - squeakers.json',
    ]),
    ('Dots', 'Sparkle', [
        'Stahlregen - Dots (Pixels - Blocky) (Jelly V2).json',
        'stahlregen - dots (layered) - suksma world humanvirus replication killphreqs mix.json',
    ]),
    ('Royal Mashup', 'Dancer', [
        f'$$$ Royal - Mashup ({n}).json' for n in (177, 197, 220, 417, 157, 253, 160)
    ]),
    ('Mig Oscilloscope', 'Waveform', [
        '_Mig_009.json', '_Mig_028.json', '_Mig_039.json', '_Mig_049.json', '_Mig_079.json',
        '_Mig_Oscilloscope008.json',
    ]),
]
# Authors of presets named without the `authors - title` pattern.
AUTHORS = {
    'Mig Oscilloscope': ['Mig'],
    'Mig Version 2': ['Mig'],
    'Mig COLORFUL9': ['Mig'],
    'Spiral Movements': ['Phat', 'Zylot', 'Eo.S.'],
}
# Family names, by first member.
NAMES = {
    '_Mig_004_version2.json': 'Mig Version 2',
    '_Mig_COLORFUL9.json': 'Mig COLORFUL9',
    'various artists - 1200774354134 - mash0000 - what the writer′s guild is doing with the extra money.json':
        'Writers Guild',
    'The NG + Flexi + BDRV - Ultramix, Aderrasi + Flexi - Predator Prey Spirals, Flexi - '
    'Jelly Showoff Parade.json': 'Predator Prey Ultramix',
    'EoS_Phat Dark_heart_nobal_cause cpe ap4+.json': 'Dark Heart',
    'phat + Eo.S. - Bass_responce_Red_Movements_Disorienting nebula3.json': 'Disorienting Nebula',
    'AdamFX Enterz Tha Mash With Martin + stahlregen - AdamFX - In Tha Mash Effectz E.json': 'In Tha Mash',
    'An AdamFX n Martin Infusion 2 flexi - Why The Sky Looks Diffrent Today - AdamFx n Martin '
    'Infusion - Tack Tile Disfunction B.json': 'Why the Sky Looks Different',
    'martin - shifter - armorial bearings of robotopia.json': 'Armorial Bearings of Robotopia',
    'one-sided infinite-dimensional shape - helenisch.json': 'One-Sided Infinite-Dimensional Shape',
    'ORB - Pastel Primer.json': 'Pastel Primer',
    'fiShbRaiN + Flexi - witchcraft 2.0.json': 'Witchcraft',
    'Rovastar & Loadus + Zylot - FractalDrop (Spark Machine v2.0).json': 'FractalDrop',
    'martin + stahlregen - martin in da mash 12.json': 'Martin in da Mash',
    'cope + martin - mother-of-pearl.json': 'Mother of Pearl',
    'Cope - The Neverending Explosion of Red Liquid Fire.json': 'The Red',
    'fed + flexi - tech test 2-1 - humpgorge.json': 'Neon Space',
    'Flexi + Geiss + Demon Lord - unholy tokamak clot-plot.json': 'Tokamak Mindblob',
    'flexi - bouncing balls [double mindblob neon mix].json': 'Bouncing Balls',
    'flexi - patternton, district of media, capitol of the united abstractions of fractopia.json': 'Patternton',
    'Phat_Zylot_Eo.S. spiral_Movements_Beatle.json': 'Spiral Movements',
    'shifter - escape (sigur ros).json': 'Escape',
    'Rovastar + Geiss - Hyperspace - kaleidoscope.json': 'Hyperspace',
    'Geiss - Cauldron - painterly (saturation remix).json': 'Cauldron',
    'Illusion & Rovastar - Dotty Mad Space (Jelly).json': 'Rovastar Jelly',
    'Zylot - In death there is life (Geiss Layered Mix) (Jelly).json': 'In Death There Is Life',
}
# Presets this dark in the 8 s render are dropped.
MIN_LIT = 0.03
PREVIEW = (192, 108)


def split_name(stem):
    """(authors, title) from a MilkDrop file name `authors - title`."""
    stem = stem.strip().lstrip('_').replace('_', ' ')
    if ' - ' in stem:
        authors, title = stem.split(' - ', 1)
    else:
        authors, title = '', stem
    names = [a.strip() for a in re.split(r'\s*(?:\+|,|&|\band\b|\bn\b)\s*', authors) if a.strip()]
    return names, title.strip()


def display_name(title):
    """Short title: the part before any `- mash...` tail, capitalized."""
    title = re.split(r'\s+-\s+', title)[0].strip()
    if len(title) > 40:
        title = title[:40].rsplit(' ', 1)[0]
    if title == title.lower():
        small = {'a', 'an', 'and', 'at', 'in', 'into', 'of', 'on', 'the', 'to', 'with'}
        words = title.split(' ')
        title = ' '.join(w if i and w in small else w[:1].upper() + w[1:] for i, w in enumerate(words))
    return title


def slug(name):
    s = re.sub(r'[^A-Za-z0-9]+', '-', name).strip('-')
    return s or 'preset'


def main():
    scratch, dest = sys.argv[1], sys.argv[2]
    clusters = json.load(open(os.path.join(scratch, 'families.json'), encoding='utf8'))
    lit = {m: s['lit'] for c in clusters for m, s in c['stats'].items()}
    cotc = {m: p for c in clusters for m, p in c['cotc'].items()}

    families = []
    for index, cluster in enumerate(clusters):
        if index in REJECT_CLUSTERS:
            continue
        members = cluster['members']
        for group in SPLITS.get(index, [list(range(len(members)))]):
            files = [members[i] for i in group]
            families.append({'category': cluster['category'], 'files': files})
    grouped = {f for _, _, files in GROUPS for f in files}
    missing = grouped - set(lit)
    if missing:
        sys.exit(f'GROUPS name presets that did not render: {sorted(missing)}')
    for family in families:
        family['files'] = [f for f in family['files'] if f not in grouped]
    for name, category, files in GROUPS:
        families.append({'category': category, 'files': list(files), 'name': name})
    for family in families:
        family['files'] = [f for f in family['files'] if f not in REJECT_FILES and lit[f] >= MIN_LIT]
    families = [f for f in families if f['files']]

    # Empty the destination, keeping the folder itself (it may be open).
    for entry in os.listdir(dest) if os.path.isdir(dest) else []:
        path = os.path.join(dest, entry)
        shutil.rmtree(path) if os.path.isdir(path) else os.remove(path)
    credits = []
    used = set()
    for family in families:
        first = family['files'][0]
        authors_all, variants = [], []
        for f in family['files']:
            authors, title = split_name(f[:-5])
            for a in authors:
                if a.lower() not in [x.lower() for x in authors_all]:
                    authors_all.append(a)
            preset = json.load(open(os.path.join(scratch, 'pool', f), encoding='utf8'))
            variants.append({'name': f[:-5], 'preset': preset})
        name = family.get('name') or NAMES.get(first) or display_name(split_name(first[:-5])[1])
        authors_all = AUTHORS.get(name, authors_all)
        category = family['category']
        base = slug(name)
        while (category, base) in used:
            base += '-2'
        used.add((category, base))
        folder = os.path.join(dest, category)
        os.makedirs(folder, exist_ok=True)
        out = {
            'name': name,
            'category': category,
            'authors': authors_all,
            'source': 'Cream of the Crop / ' + ', '.join(sorted({cotc[f] for f in family['files']})),
            'variants': variants,
        }
        with open(os.path.join(folder, base + '.json'), 'w', encoding='utf8') as fh:
            json.dump(out, fh, ensure_ascii=False, separators=(',', ':'))
        png = os.path.join(scratch, 'pool-render', first[:-5] + '-0.png')
        Image.open(png).convert('RGB').resize(PREVIEW, Image.LANCZOS).save(
            os.path.join(folder, base + '.jpg'), quality=82, optimize=True)
        credits.append((category, name, [v['name'] for v in variants]))

    count = sum(len(c[2]) for c in credits)
    lines = [
        '# MilkDrop presets bundled with Verso',
        '',
        f'{len(credits)} preset families, {count} presets, picked from the',
        '[Cream of the Crop](https://github.com/projectM-visualizer/presets-cream-of-the-crop)',
        'collection that ISOSCELES curated for NestDrop, in the GLSL conversion of the',
        '[Butterchurn presets](https://github.com/jberg/butterchurn-presets) (MIT).',
        'Categories are the Cream of the Crop folders. Presets that are remixes of',
        'each other are grouped as the variants of one family.',
        '',
        'MilkDrop presets were released freely by their authors, without a specific',
        'license; the author credits are the preset names below. Authors who want a',
        'preset removed can open an issue.',
        '',
    ]
    for category in sorted({c[0] for c in credits}):
        lines.append(f'## {category}')
        lines.append('')
        for cat, name, variants in sorted(credits):
            if cat != category:
                continue
            lines.append(f'- **{name}**: ' + '; '.join(variants))
        lines.append('')
    with open(os.path.join(dest, 'CREDITS.md'), 'w', encoding='utf8') as fh:
        fh.write('\n'.join(lines))
    print(len(credits), 'families,', count, 'presets')


if __name__ == '__main__':
    main()
