# Verso

Free and open source VJ software. Clip grid of layers × columns, GPU compositing
with blend modes, live output window. Written in Rust on wgpu, for Windows, macOS
and Linux.

> Status: v0.3. Generative sources, images and video (any codec FFmpeg reads,
> including HAP and ProRes), effect chains (WGSL and ISF) on clips, layers,
> groups and the composition, clip transitions and layer groups. OSC and MIDI
> control with MIDI learn, audio analysis, tap tempo, LFO or audio modulation and
> Lua scripts.
> See [docs/SPEC.md](docs/SPEC.md) (French) for the architecture and roadmap.

## Requirements

- Rust (stable)
- FFmpeg (`ffmpeg` and `ffprobe`) on `PATH` for video, or set `VERSO_FFMPEG` and
  `VERSO_FFPROBE` to their paths

## Run

```bash
cargo run --release -p verso-app
```

## Controls

| Input | Action |
|---|---|
| Click a cell | Trigger that clip on its layer (empty cell ejects the layer) |
| Click a column header, or keys `1`–`9` | Trigger the whole column |
| Drag a source from the Sources browser onto a cell | Load it into that slot |
| Drop image or video files from your file manager onto a cell | Load them into that slot and the next ones |
| Browser › Files › Open Folder… | List a folder's media to drag onto cells |
| Click the playing clip again | Restart it |
| Drag an effect from Browser › Effects onto a clip, layer header or Composition header | Add it to that chain |
| Properties › + Add Effect | Add an effect to the selected composition, group, layer or clip |
| Layer › Group Selected Layer | Put the selected layer in a new group |
| Right-click a cell | Change its source or clear it |
| `X` / `B` / `S` on a layer | Eject / bypass / solo |
| `A` / `B` on a layer | Assign it to a crossfader side |
| Double-click a fader | Reset it |
| `L` or **MIDI Learn** | Learn mode: click a control, then move a MIDI control |
| `T` or **TAP** | Tap tempo |
| Right-click a fader or effect parameter | Add an LFO or audio modulation |
| **Control…** | OSC port, MIDI inputs and mappings, audio input, modulators |
| **Scripts…** | Write Lua automations (Ctrl+Enter applies) |
| `Esc` | Clear all layers |
| `F2` | Toggle the output window (double-click it for fullscreen) |

## Workspace

| Crate | Role |
|---|---|
| `verso-core` | Composition model and commands, no GPU dependency |
| `verso-media` | Image decoding and FFmpeg video playback, no GPU dependency |
| `verso-control` | OSC, MIDI, audio analysis and tap tempo |
| `verso-script` | Sandboxed Lua 5.4 scripts |
| `verso-render` | wgpu engine: sources, media, blend modes, master |
| `verso-app` | eframe/egui application |

## Effects

Verso loads `.wgsl` (Verso format) and `.fs` (ISF) files from an `effects` folder
in the working directory or next to the executable, and from any folder picked
in Browser › Effects › Load Folder…. See [effects/wave.wgsl](effects/wave.wgsl)
and [effects/scanlines.fs](effects/scanlines.fs) for one example of each.

## OSC

Listen from the Control window (default port 7000). Values are 0–1; indices
start at 1. Examples: `/layer/2/opacity 0.5`, `/column/3/launch`,
`/layer/1/effect/1/radius 0.8`. The full list is in [docs/SPEC.md](docs/SPEC.md).

## Scripts

Lua scripts define `frame(t)` and/or `beat(n)` and drive the composition with
`verso.set(address, value)`, `verso.trigger(address)` and `verso.get(address)`,
using the OSC addresses. See [scripts/beat-chase.lua](scripts/beat-chase.lua).
Scripts are saved in the composition and run sandboxed (no file or OS access,
endless loops are stopped).

## Test

```bash
cargo test --workspace
```

GPU tests in `verso-render/tests` check output pixels and write
`target/verso-demo.png`. They skip when no GPU adapter is available.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Verso is an independent clean-room project. It is not affiliated with, and does
not reuse code or assets from, any commercial VJ software.
