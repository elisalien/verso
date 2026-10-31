//! Translates the warp and comp shaders of every Butterchurn preset in a
//! folder: `cargo run --release -p verso-render --example milk_shaders -- <dir>`
use verso_milkdrop::Preset;
use verso_render::milkdrop::glsl::{Stage, translate};

fn main() {
    let dir = std::env::args().nth(1).expect("usage: milk_shaders <dir>");
    let (mut ok, mut failed) = (0, 0);
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let Ok(preset) = Preset::from_json(&std::fs::read_to_string(&path).unwrap()) else {
            continue;
        };
        let mut good = true;
        for (stage, text) in [(Stage::Warp, &preset.warp), (Stage::Comp, &preset.comp)] {
            if let Err(e) = translate(stage, text) {
                good = false;
                let first = e.lines().find(|l| l.contains("error")).unwrap_or("").to_owned();
                println!("{stage:?} {name}: {first}");
            }
        }
        if good { ok += 1 } else { failed += 1 }
    }
    println!("ok {ok}, failed {failed}");
}
