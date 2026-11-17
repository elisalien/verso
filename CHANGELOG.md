# Changelog

All notable changes to Verso. Versions follow [Semantic Versioning](https://semver.org).
Full release notes, in English and French, are in [docs/releases](docs/releases).

## [0.3.0] - 2026-11-17

First public release, with packages for Windows, macOS and Linux. Experimental; the macOS build is untested.

### Added

- Clip grid of layers and columns, A/B crossfader, solo, bypass, nested layer groups, clip transitions.
- Generators, images and video through FFmpeg (HAP, DXV3, ProRes, H.264, H.265), video transport.
- Native MilkDrop engine with 147 preset families, blending between presets and between clips, video input.
- WGSL and ISF effect chains on clips, layers, groups and the composition; online ISF and ShaderToy browser.
- Advanced Output: screens, slices, polygon masks, warp grids, soft edge blending, test patterns, Identify, Arena preset import and export; NDI output.
- Cue list with Go, fades and follow-on.
- Favorites on sources, MilkDrop presets, effects and media, with a Favorites tab in the browser; right-click a clip in the grid or an effect in the Properties to star its source or effect.
- Layer headers show a thumbnail of the playing clip; clicking it selects the clip and scrolls the grid to it.
- MIDI learn, OSC (including the `/composition/…` scheme), audio analysis, tap tempo, LFO and audio modulation.
- Sandboxed Lua scripts.
- Docking panels, layouts saved with compositions, undo/redo, autosave, log file.
- Opt-in MCP server for agent control.
- FFmpeg is looked up next to the executable before `PATH`, so release archives can ship it.

### Changed

- Verso opens the last composition at startup, or an empty one; the demo composition is gone.
- Only one Verso runs at a time: a second start says so and quits.
- The first mapping model (`mapping`, `.versomap`) is removed; Advanced Output replaces it.

### Fixed

- Presets, movies and output presets from other people can no longer freeze or crash Verso: MilkDrop equations have a loop budget per frame and a nesting limit, wave sample counts, MOV box sizes and frame sizes, and warp grid sizes are bounded.
- An ISF file whose header comment opens with `/*/` no longer crashes Verso at every start.
- Switching video clips no longer leaks a GPU texture each time.
- A missing or broken video is no longer reopened on every frame; one unreadable HAP frame is skipped.
- A MilkDrop preset whose equations fail is no longer restarted on every frame.
- Undo and redo of a layer or column removal, move or duplicate keep players, selection, locked panels and Advanced Output slices on the right layers.
- Scripts that share a name no longer replace each other on every frame.
- Scrubbing an FFmpeg-decoded video, hovering a header for the first time, opening a folder of MilkDrop presets, the recovery copy and preference sliders no longer stall the output.
- YouTube: one download per video, downloads stop when Verso closes.
- Garbled characters in the MilkDrop clip properties.
- A shader that declares a parameter range backwards no longer closes Verso when the parameter is moved by MIDI, OSC, a script or a modulator.
- Pictures larger than 8192 pixels are scaled down instead of showing black.
- Ejecting or replacing an NDI or Spout clip no longer stalls the output for up to a quarter of a second.
- Deleting a layer keeps MilkDrop clips reading the right input layer; reordering layers no longer restarts the MilkDrop presets playing on them.
- A video that was still opening when a layer was moved, duplicated or deleted no longer stays on its poster image.
- "Settings…" in the MilkDrop browser tab opens the MilkDrop Settings panel.
- Files with invalid cue values, group parents or text styles are repaired on opening.
- Dragging in Advanced Output or MilkDrop Settings no longer writes the settings file during the drag; settings files can no longer be left half-written.
- A Spout program that crashed while holding the sender list no longer makes Verso block the other Spout programs.
- The MCP server refuses endless header lines; OSC messages with oversized addresses are dropped.
- A non-looping HAP clip reports its end in the transport.
- Importing a shader or loading an effects folder during a show recompiles only the effects that changed, not every effect of the composition.
- The first launch of a MilkDrop preset compiles each of its shaders once instead of twice; compiled presets are kept for the 48 used most recently.
- Relaunching or scrubbing an FFmpeg-decoded video no longer waits for a process on the render thread: the current frame stays until the new position is ready.
- An audio input that is unplugged goes silent instead of freezing its last levels, and is started again when it is back; "Default output" follows the output Windows switches to.
- MilkDrop presets and clip transitions stay smooth after hours of running (their timing no longer loses precision).
- After a crash, the Spout output can use its name again while a receiver is still open.
- Dragging a slider that changes many clips at once makes one undo step per clip instead of emptying the undo history.
- Layers that show nothing no longer hold four full-size textures each.
- Windows: Verso no longer loads the OpenGL driver next to DirectX 12 and Vulkan. With some Intel drivers it crashed the process as it closed.

### Security

- `VERSO_MCP_TOKEN`: when set, the MCP server only answers clients that send it as a bearer token. The MCP `save` tool only writes `.verso` files.

[0.3.0]: https://github.com/elisalien/verso/releases/tag/v0.3.0
