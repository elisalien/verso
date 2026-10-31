import os, re, json, sys, collections, shutil
conv, cotc, out = sys.argv[1], sys.argv[2], sys.argv[3]
def norm(s):
    s = s.lower().replace('.milk','').replace('.json','').replace('\u2032',"'").replace('_',' ')
    return re.sub(r'[^a-z0-9]+', '', s)
index = {}
for root, _, files in os.walk(cotc):
    for f in files:
        if f.endswith('.milk'):
            index.setdefault(norm(f), os.path.relpath(root, cotc).replace(os.sep, '/'))
STOP = r'\b(mix|remix|rmx|edit|version|ver|v\d+|nz\+?|mash\w*|update|lite|optimi[sz]ed?|fix(ed)?)\b'
def core(name):
    stem = name[:-5]
    parts = [p.strip() for p in re.split(r'\s+-\s+', stem)]
    title = parts[1] if len(parts) > 1 else parts[0]
    title = re.sub(r'[\(\[].*?[\)\]]', ' ', title)
    title = title.lower()
    title = re.sub(STOP, ' ', title)
    title = re.sub(r'[\d.]+', ' ', title)
    title = re.sub(r'[^a-z]+', ' ', title).strip()
    return title or stem.lower()
groups = collections.defaultdict(list)
for f in sorted(os.listdir(conv)):
    cat = index.get(norm(f))
    if not cat: continue
    groups[core(f)].append({'file': f, 'cotc': cat})
os.makedirs(out, exist_ok=True)
for key, items in groups.items():
    for it in items:
        shutil.copy(os.path.join(conv, it['file']), os.path.join(out, it['file']))
json.dump(groups, open('groups.json', 'w', encoding='utf8'), indent=1, ensure_ascii=False)
sizes = collections.Counter(len(v) for v in groups.values())
print(len(groups), 'groups; sizes', sorted(sizes.items()))
for k, v in sorted(groups.items(), key=lambda kv: -len(kv[1]))[:25]:
    print(f'{len(v):2} {k!r}: ' + ' | '.join(i['file'][:40] for i in v[:4]))
