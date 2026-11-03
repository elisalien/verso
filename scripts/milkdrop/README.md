# MilkDrop preset selection

How `presets/milkdrop` was built. Work in a scratch folder (here `$S`).

1. Get the sources:
   - `git clone --depth 1 https://github.com/projectM-visualizer/presets-cream-of-the-crop $S/cotc`
   - `git clone --depth 1 https://github.com/jberg/butterchurn-presets $S/ref/butterchurn-presets`
     (needs `git -c core.longpaths=true` on Windows)
2. Keep the Butterchurn presets that are also in Cream of the Crop, with a
   first grouping by title: `python -X utf8 group.py $S/ref/butterchurn-presets/presets/converted $S/cotc $S/pool`
   (run from `$S`; writes `groups.json`).
3. Render every candidate:
   `cargo run --release -p verso-render --example milk_render -- --out $S/pool-render --seconds 8 $S/pool > $S/pool-render.txt`
4. Cluster remixes by code similarity: `python -X utf8 cluster.py pool pool-render.txt families.json` (from `$S`).
5. Look at the contact sheets: `python -X utf8 sheets.py` (from `$S`, writes `sheets/`).
6. Encode the choices in `curate.py` (rejects, splits, regroupings, names),
   then write the bundle: `python -X utf8 curate.py $S presets/milkdrop`.

`cargo test -p verso-render bundled_presets_compile` checks that every bundled
preset still compiles.
