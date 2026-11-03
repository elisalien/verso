import os, re, json, sys, collections
pool, renders, out = sys.argv[1], sys.argv[2], sys.argv[3]
groups = json.load(open('groups.json', encoding='utf8'))
cat_of = {i['file']: i['cotc'] for v in groups.values() for i in v}
name_key = {i['file']: k for k, v in groups.items() for i in v}
stats = {}
for line in open(renders, encoding='utf8'):
    p = line.split()
    if p[0] == 'ok':
        png = line[3:line.rindex('.png') + 4]
        f = os.path.basename(png)[:-6] + '.json'
        lit, change, ms = map(float, line[line.rindex('.png') + 4:].split())
        stats[f] = dict(lit=lit, change=change, ms=ms)
def shingles(d):
    code = ' '.join([d.get('frame_eqs_eel',''), d.get('pixel_eqs_eel',''), d.get('warp',''), d.get('comp','')]
        + [s.get('frame_eqs_eel','') for s in d.get('shapes',[])] + [w.get('point_eqs_eel','') for w in d.get('waves',[])])
    code = re.sub(r'//[^\n]*', ' ', code)
    toks = re.findall(r'[a-z_][a-z0-9_]*|\d+\.?\d*|\S', code.lower())
    toks = [t for t in toks if not re.fullmatch(r'tmpvar_\d+|\d+\.?\d*', t)]
    return {' '.join(toks[i:i+6]) for i in range(len(toks) - 5)}
files = sorted(f for f in os.listdir(pool) if f in stats)
sh = {f: shingles(json.load(open(os.path.join(pool, f), encoding='utf8'))) for f in files}
parent = {f: f for f in files}
def find(f):
    while parent[f] != f:
        parent[f] = parent[parent[f]]; f = parent[f]
    return f
for i, a in enumerate(files):
    for b in files[i+1:]:
        same_name = name_key[a] == name_key[b]
        A, B = sh[a], sh[b]
        j = len(A & B) / max(1, len(A | B))
        if same_name or j > 0.45:
            parent[find(a)] = find(b)
fam = collections.defaultdict(list)
for f in files: fam[find(f)].append(f)
result = []
for members in fam.values():
    cats = collections.Counter(cat_of[m].split('/')[0] for m in members)
    result.append(dict(category=cats.most_common(1)[0][0], members=sorted(members),
                       stats={m: stats[m] for m in members}, cotc={m: cat_of[m] for m in members}))
result.sort(key=lambda r: (r['category'], r['members'][0].lower()))
json.dump(result, open(out, 'w', encoding='utf8'), indent=1, ensure_ascii=False)
print(len(result), 'families', collections.Counter(len(r['members']) for r in result))
print(collections.Counter(r['category'] for r in result))
for r in result:
    if len(r['members']) > 1: print(r['category'], '|', ' || '.join(m[:45] for m in r['members']))
