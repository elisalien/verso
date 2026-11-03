# Verso

Free and open source VJ software. Clip grid of layers × columns, GPU compositing
with blend modes, live output window. Written in Rust on wgpu, for Windows, macOS
and Linux.

> Status: v0.4.1. Generative sources, images and video (any codec FFmpeg reads,
> including HAP and ProRes), effect chains (WGSL and ISF) on clips, layers,
> groups and the composition, clip transitions and layer groups. OSC and MIDI
> control with MIDI learn, audio analysis, tap tempo, LFO or audio modulation and
> Lua scripts. Ableton Link, MIDI and audio (LTC) timecode with cues, NDI® input
> and output, Spout (Windows) and Syphon (macOS) output. HAP plays natively on
> the GPU (instant seeking, reverse playback) and outgoing clips keep playing
> during transitions.
> See [docs/SPEC.md](docs/SPEC.md) (French) for the architecture and roadmap.

## Requirements

- Rust (stable)
- FFmpeg (`ffmpeg` and `ffprobe`) on `PATH` for video, or set `VERSO_FFMPEG` and
  `VERSO_FFPROBE` to their paths
- CMake and a C++ compiler for Ableton Link (build without it:
  `cargo build --release -p verso-app --no-default-features`)
- Linux: ALSA headers to build (`sudo apt install libasound2-dev pkg-config` on
  Debian/Ubuntu, `alsa-lib-devel` on Fedora)

## Run

```bash
cargo run --release -p verso-app
```

## Controls

| Input | Action |
|---|---|
| Click a cell | Trigger that clip on its layer (empty cell ejects the layer). Preferences can make click select and double-click launch |
| Ctrl+click a cell | Select it without launching |
| Click a column header, or keys `1`–`9` (`Shift+1`–`9` for columns 10–18) | Trigger the whole column |
| Drag a cell onto another | Move the clip (swaps with a full cell); hold `Alt` to copy |
| Drag a source from the Sources browser onto a cell | Load it into that slot |
| Drop image or video files from your file manager onto a cell | Load them into that slot and the next ones |
| Browser › Files › Open Folder… | List a folder's media to drag onto cells |
| Click the playing clip again | Restart it |
| Drag an effect from Browser › Effects onto a clip, layer header or Composition header | Add it to that chain |
| Properties › + Add Effect | Add an effect to the selected composition, group, layer or clip |
| Right-click a layer or column header | Duplicate, move, delete (layers); rename, move, delete (columns) |
| Layer › Group Selected Layer | Put the selected layer in a new group |
| Right-click a cell | Copy, paste, change its source or clear it |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` / `Del` | Copy, cut, paste, clear the selected cell |
| `Ctrl+Z` / `Ctrl+Shift+Z` (or `Ctrl+Y`) | Undo / redo edits (launching clips is never undone) |
| `Ctrl+N` / `Ctrl+O` / `Ctrl+S` / `Ctrl+Shift+S` | New, open, save, save as |
| `Ctrl+D` | Duplicate the selected layer |
| `Ctrl+Shift+L` | Live lock: only launching and performance controls work |
| `X` / `B` / `S` on a layer | Eject / bypass / solo |
| `A` / `B` on a layer | Assign it to a crossfader side |
| Double-click a fader | Reset it |
| `L` or **MIDI Learn** | Learn mode: click a control, then move a MIDI control |
| `T` or **TAP** | Tap tempo |
| Right-click a fader or effect parameter | Add an LFO or audio modulation |
| **Control…** | OSC port and access, MIDI inputs and mappings, audio input, modulators |
| **Scripts…** | Write Lua automations (Ctrl+Enter applies) |
| `Shift+Esc` | Clear all layers (plain `Esc` only closes menus) |
| `F2` | Toggle the output window (double-click it for fullscreen) |
| Output › NDI® Output | Publish the output as an NDI source (needs NDI Tools or the NDI runtime) |
| Output › Spout / Syphon Output | Share the output with apps on the same computer (Windows: Spout; macOS: Syphon, needs `Syphon.framework` in `/Library/Frameworks`) |
| Browser › Sources › NDI® sources › Find | List NDI senders on the network; drag one onto a cell to play it live |
| **LINK** | Join the Ableton Link session: shared tempo and beat phase (number of peers shown) |
| Control › Timecode | Read MIDI Time Code or LTC on an audio channel; add cues that fire an OSC address at a timecode |
| Browser › Online | Search and import ISF and ShaderToy shaders (ShaderToy needs your free API key) |
| Cell badge | Times that slot was launched; right-click › Reset Counter |
| Properties › Clip › Position / Speed | Scrub a playing video; HAP files also play backwards (negative speed) |
| ⚠ in the status bar | Problems of the session (media, effects, GPU, repaired files) |
| Composition › Preferences… | Click behaviour, interface size, recovery copy, reopen last file |

Unsaved work is copied every minute and offered back after a crash. Missing
media files can be relinked by picking a folder when a composition opens.

## Workspace

| Crate | Role |
|---|---|
| `verso-core` | Composition model and commands, no GPU dependency |
| `verso-media` | Image decoding and FFmpeg video playback, no GPU dependency |
| `verso-control` | OSC, MIDI, audio analysis and tap tempo |
| `verso-script` | Sandboxed Lua 5.4 scripts |
| `verso-io` | NDI output, runtime loaded at run time |
| `verso-online` | ISF and ShaderToy search and import |
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

Imported shaders are saved in Verso's user folder (`%APPDATA%\Verso\shaders` on
Windows). Generators (no input image) appear under Sources; filters under
Effects. ShaderToy shaders are CC BY-NC-SA 3.0 unless their code says
otherwise: use the "Commercial use only" filter and check the shader page.

## Scripts

Lua scripts define `frame(t)` and/or `beat(n)` and drive the composition with
`verso.set(address, value)`, `verso.trigger(address)` and `verso.get(address)`,
using the OSC addresses. `t.timecode` gives the incoming timecode in seconds.
See [scripts/beat-chase.lua](scripts/beat-chase.lua).
Scripts are saved in the composition and run sandboxed (no file or OS access,
endless loops are stopped).

NDI® is a registered trademark of Vizrt NDI AB.

## Safety

- Scripts saved in a composition run sandboxed: no file, OS or network
  access, 64 MiB of memory and 5 million instructions per call (`pcall` cannot
  catch the limit).
- Compositions are checked when opened: inconsistent or out-of-range values
  are repaired and listed in the status bar. Saves are atomic and keep the
  previous version as `<name>.verso.bak`.
- Media paths are read as local files only.
- OSC listens to this computer only unless **Accept from other computers** is
  on in the Control window; you can then list the allowed addresses.
- Logs and crash reports go to `verso.log` and `crash.log` in Verso's user
  folder (`%APPDATA%\Verso`, `~/Library/Application Support/Verso` or
  `~/.config/verso`).

## Test

```bash
cargo test --workspace
```

## Bench

```bash
cargo run --release -p verso-app --example bench
```

Measures compositing (1, 4, 8 layers), FFmpeg decoding of 1080p H.264 and HAP
(1 and 4 players) and the full path of 4 HAP 1080p layers against 60 fps. Run it
on the show computer.

GPU tests in `verso-render/tests` check output pixels and write
`target/verso-demo.png`. They skip when no GPU adapter is available.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Verso is an independent clean-room project. It is not affiliated with, and does
not reuse code or assets from, any commercial VJ software.
