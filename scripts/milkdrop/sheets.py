import os, json, sys
from PIL import Image, ImageDraw
fams = json.load(open('families.json', encoding='utf8'))
W, H, COLS, ROWS = 240, 135, 5, 6
cells = []
for fi, f in enumerate(fams):
    for vi, m in enumerate(f['members']):
        cells.append((f"F{fi} {f['category'][:4]} v{vi} {m[:-5]}"[:40], os.path.join('pool-render', m[:-5] + '-0.png'), f['stats'][m]))
os.makedirs('sheets', exist_ok=True)
per = COLS * ROWS
for s in range(0, len(cells), per):
    sheet = Image.new('RGB', (COLS * W, ROWS * (H + 12)), (40, 40, 40))
    d = ImageDraw.Draw(sheet)
    for i, (label, png, st) in enumerate(cells[s:s + per]):
        x, y = (i % COLS) * W, (i // COLS) * (H + 12)
        if os.path.exists(png):
            sheet.paste(Image.open(png).convert('RGB').resize((W - 2, H)), (x, y + 12))
        d.text((x + 1, y), label, fill='white')
    sheet.save(f'sheets/sheet{s // per:02}.png')
print(len(cells), 'cells', (len(cells) + per - 1) // per, 'sheets')
