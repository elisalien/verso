# Verso

Free and open source VJ software. Clip grid of layers × columns, GPU compositing
with blend modes, live output window. Written in Rust on wgpu, for Windows, macOS
and Linux.

> Status: v0.3. Generative sources, images and video (any codec FFmpeg reads,
> including HAP, Resolume DXV3 and ProRes), effect chains (WGSL and ISF) on
> clips, layers, groups and the composition, clip transitions and layer groups.
> OSC and MIDI control with MIDI learn, audio analysis, tap tempo, LFO or audio
> modulation and Lua scripts.
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
| Ctrl/Shift-click layer headers, then `Ctrl+G` | Group adjacent layers; grouping a group with its neighbours nests it |
| `Ctrl+Shift+G` | Ungroup: contents move up one level |
| Group header ▸/▾ or double-click | Collapse or expand the group; ▶ buttons launch a column in that group only |
| Right-click a panel tab › Undock | Open the panel in its own window (Dock back or close it to return) |
| Hover a clip thumbnail | Show it in the Preview panel |
| Window › Interface Colors… | Pick a preset or any color for each interface role |
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
| Window › Cues | Cue list: **Record** stores the clip playing and the opacity of every layer, the master and the crossfader; **GO** or `Space` plays the next cue with its fade (clips crossfade, levels move); each cue waits for Go or follows on after some seconds or beats. Moving a fader by hand stops a fade |
| `Ctrl+N` / `Ctrl+O` / `Ctrl+S` / `Ctrl+Shift+S` | New, open, save, save as. Verso asks before losing unsaved changes (`*` in the title) |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / redo structure edits: clips, layers, columns, groups, effect chains, mappings (fader moves are not undone) |
| Right-click a layer header › Delete Layer, a column header › Delete Column | Remove them with their clips (undoable) |
| Click the status bar, or Window › Messages… | History of status messages and errors |
| `F2` | Toggle the output window (double-click it for fullscreen) |
| Output › Advanced Output… | Screens and slices as in Resolume Arena: Input Selection and Output Transformation views (drag slices, corners, polygon points, and warp grid points), polygons drawn by clicking or imported from SVG, a monitor per display screen. Import and export Arena presets (`.xml`), or drop one on Verso. Live renders every screen: full screen on its monitor, or an NDI source |
| Output › NDI® Output | Publish the output as an NDI source (needs NDI Tools or the NDI runtime) |
| Drag a panel tab | Move it to another tab group, or to an edge to split; drag separators to resize |
| Window menu | Open more Composition, Group, Layer or Clip panels, reopen panels, reset the layout |
| 🔗 Follow / 🔒 Locked in a Properties panel | Follow the selection, or stay on one group, layer or clip; the list picks the target |
| Browser › Online | Search and import ISF and ShaderToy shaders (ShaderToy needs your free API key) |
| Browser › Sources › MilkDrop | Audio-reactive MilkDrop presets by category; a clip's Properties pick among the remixes of its preset (with a MilkDrop blend) and can feed another layer into it |
| Cell badge | Times that slot was launched; right-click › Reset Counter |
| Hover a layer or column header, or a MilkDrop clip | The Preview monitor shows that layer alone, what the column would output, or the clip playing live |
| Hover a source in Browser › Sources | The Preview monitor plays it; List / Thumbnails switches the browser to a grid of live thumbnails |
| Window › Save Layout as Default | Reset Layout then returns to this layout (Built-in Layout restores the original) |

## Workspace

| Crate | Role |
|---|---|
| `verso-core` | Composition model and commands, no GPU dependency |
| `verso-media` | Image decoding and FFmpeg video playback, no GPU dependency |
| `verso-control` | OSC, MIDI, audio analysis and tap tempo |
| `verso-script` | Sandboxed Lua 5.4 scripts |
| `verso-io` | NDI output, runtime loaded at run time |
| `verso-online` | ISF and ShaderToy search and import |
| `verso-milkdrop` | MilkDrop presets: EEL interpreter, frame engine, audio analysis |
| `verso-render` | wgpu engine: sources, media, blend modes, master |
| `verso-app` | eframe/egui application |

## Files

Verso keeps its own files in a user folder: `%APPDATA%\Verso` on Windows,
`~/.config/verso` on Linux, `~/Library/Application Support/Verso` on macOS.

| File | Content |
|---|---|
| `layout.ron` | Panel layout of the last session |
| `verso.log`, `verso.log.old` | Log of this run and the previous one: status messages, GPU and window warnings, crashes. Open Window › Messages to see the path. |
| `Untitled.verso.bak` | Autosave of a composition that has no file yet (`Untitled-mcp-<port>.verso.bak` for an instance started with `--mcp`, which also backs up a saved composition to `name.mcp-<port>.verso.bak`) |
| `shaders/`, `milkdrop/` | Imported shaders and your own MilkDrop presets |

A modified composition is also copied every two minutes to `name.verso.bak`
next to its file; saving removes the copy.

A `.verso` file also stores the panel layout it was saved with (docking,
floating panels, locked Properties panels). Opening it brings that layout
back; a file saved before this, or a new composition, keeps the current one.

## Effects

Verso loads `.wgsl` (Verso format) and `.fs` (ISF) files from an `effects` folder
in the working directory or next to the executable, and from any folder picked
in Browser › Effects › Load Folder…. See [effects/wave.wgsl](effects/wave.wgsl)
and [effects/scanlines.fs](effects/scanlines.fs) for one example of each.

## OSC

Listen from the Control window (default port 7000). Values are 0–1; indices
start at 1. Examples: `/layer/2/opacity 0.5`, `/column/3/launch`,
`/layer/1/effect/1/radius 0.8`, `/cue/go`, `/cue/previous`, `/cue/2/go`. The
full list is in [docs/SPEC.md](docs/SPEC.md).

Verso also answers the `/composition/…` address scheme that common clip-grid
VJ software and the OSC presets made for it use, so a TouchOSC layout, a
Chataigne module or a controller template built for that scheme drives Verso
unchanged:

| Address | Verso action |
|---|---|
| `/composition/layers/N/clips/M/connect` | Launch clip (layer N from the bottom, column M) |
| `/composition/columns/M/connect` | Launch column |
| `/composition/groups/G/columns/M/connect` | Launch column in group G |
| `/composition/layers/N/clear` | Eject layer |
| `/composition/disconnectall` | Clear all layers |
| `/composition/layers/N/video/opacity`, `/composition/groups/G/video/opacity` | Opacity 0–1 |
| `/composition/layers/N/bypassed`, `/solo`, `/composition/groups/G/bypassed` | Toggles (above 0.5 = on) |
| `/composition/layers/N/transition/duration` | Transition, in seconds |
| `/composition/master` | Master opacity 0–1 |
| `/composition/crossfader/phase` | Crossfader 0–1 |
| `/composition/tempocontroller/tempo` | Tempo, in BPM |

Only addresses documented in public user manuals are mapped; effect
parameters keep Verso's own `/layer/N/effect/I/<param>` form.

Imported shaders are saved in Verso's user folder (`%APPDATA%\Verso\shaders` on
Windows). Generators (no input image) appear under Sources; filters under
Effects. ShaderToy shaders are CC BY-NC-SA 3.0 unless their code says
otherwise: use the "Commercial use only" filter and check the shader page.

## MCP (agent control)

Verso can run a [Model Context Protocol](https://modelcontextprotocol.io)
server so an agent such as Claude Code reads the composition, drives it and
gets GPU-rendered images, without screenshots or clicks. It is off unless
asked for at launch, listens on `127.0.0.1` only, and refuses destructive
actions (deleting layers or columns, open, save, new) without
`"confirm": true`. Design and tool list: [docs/MCP.md](docs/MCP.md).

Start Verso with the server (default port 7400), or set `VERSO_MCP_PORT`:

```bash
target/release/verso --mcp 7400
```

Then register it in Claude Code, once:

```bash
claude mcp add --transport http verso http://127.0.0.1:7400/mcp
```

`claude mcp get verso` should say "Connected" while Verso runs. Only the
instance started with `--mcp` answers, so a second Verso window open next to
it is never driven by mistake; the `status` tool gives its process id.

Tools: `status`, `get_composition`, `get_layer`, `get_clip`, `list_sources`,
`list_effects`, `get_log` (reading); `osc` (any address of the OSC section
above), `set_clip`, `set_milkdrop`, `add_effect`, `apply_commands`, `select`,
`hover`, `undo`, `redo`, `save`, `open`, `new_composition` (acting, through
the same commands as the interface, so undo and the unsaved marker follow);
`capture` (PNG of the whole window, the output, a layer, a column, a clip, a
source not placed anywhere or the hover preview); `ui_widgets` and `ui_input`
(real hovers, clicks, drags, wheel and keys in the window, in UI points), so
an agent can test the interface itself. Indices start at 1, as in OSC. An
instance with the server logs to `verso-mcp-<port>.log` instead of
`verso.log`.

## MilkDrop

Verso runs MilkDrop presets natively (a port of
[Butterchurn](https://github.com/jberg/butterchurn)), reacting to the audio
input. 147 preset families ship in `presets/milkdrop`, picked from the
[Cream of the Crop](https://github.com/projectM-visualizer/presets-cream-of-the-crop)
collection curated for NestDrop; remixes of one preset are grouped as one
source with several presets. Credits: [presets/milkdrop/CREDITS.md](presets/milkdrop/CREDITS.md).
Changing the preset of a clip blends like MilkDrop (Blend, in seconds), and so
does launching another MilkDrop clip on the same layer: the new preset takes
over the image the previous one left, instead of the layer's crossfade. A
clip's Input feeds another layer into the preset, as its image textures and
mixed into its feedback (Input Mix), like NestDrop's video input; set that
layer's opacity to 0 to use it only as an input.
Add your own presets as Butterchurn `.json` files in `%APPDATA%\Verso\milkdrop`.
Raw `.milk` files are not supported yet.

## Scripts

Lua scripts define `frame(t)` and/or `beat(n)` and drive the composition with
`verso.set(address, value)`, `verso.trigger(address)` and `verso.get(address)`,
using the OSC addresses. See [scripts/beat-chase.lua](scripts/beat-chase.lua).
Scripts are saved in the composition and run sandboxed (no file or OS access,
endless loops are stopped).

NDI® is a registered trademark of Vizrt NDI AB.

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
