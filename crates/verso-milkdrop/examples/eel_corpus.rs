//! Compiles and runs every Butterchurn preset in a folder:
//! `cargo run --release -p verso-milkdrop --example eel_corpus -- <dir>`
use std::time::Instant;

use verso_milkdrop::{AudioState, Engine, Preset};

fn main() {
    let dir = std::env::args().nth(1).expect("usage: eel_corpus <dir>");
    let mut ok = 0;
    let mut failed = 0;
    let mut slowest = (0.0f64, String::new());
    let samples: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let preset = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Preset::from_json(&t)) {
            Ok(p) => p,
            Err(e) => {
                println!("PARSE {name}: {e}");
                failed += 1;
                continue;
            }
        };
        match Engine::new(&preset, 1280, 720, 3) {
            Ok(mut engine) => {
                let mut audio = AudioState::new(44100.0);
                let start = Instant::now();
                for frame in 1..=30u64 {
                    audio.update(&samples, 60.0, frame);
                    engine.frame(&audio, frame as f64 / 60.0, 60.0, frame);
                }
                let ms = start.elapsed().as_secs_f64() * 1000.0 / 30.0;
                if ms > slowest.0 {
                    slowest = (ms, name.clone());
                }
                ok += 1;
            }
            Err(e) => {
                println!("EEL {name}: {e}");
                failed += 1;
            }
        }
    }
    println!("ok {ok}, failed {failed}, slowest {:.2} ms/frame: {}", slowest.0, slowest.1);
}
