# Changelog

All notable changes to Verso. Versions follow [Semantic Versioning](https://semver.org).
Full release notes, in English and French, are in [docs/releases](docs/releases).

## [0.3.0] - unreleased

First public release, with packages for Windows, macOS and Linux.

### Added

- Clip grid of layers and columns, A/B crossfader, solo, bypass, nested layer groups, clip transitions.
- Generators, images and video through FFmpeg (HAP, DXV3, ProRes, H.264, H.265), video transport.
- Native MilkDrop engine with 147 preset families, blending between presets and between clips, video input.
- WGSL and ISF effect chains on clips, layers, groups and the composition; online ISF and ShaderToy browser.
- Advanced Output: screens, slices, polygon masks, warp grids, soft edge blending, test patterns, Identify, Arena preset import and export; NDI output.
- Cue list with Go, fades and follow-on.
- MIDI learn, OSC (including the `/composition/…` scheme), audio analysis, tap tempo, LFO and audio modulation.
- Sandboxed Lua scripts.
- Docking panels, layouts saved with compositions, undo/redo, autosave, log file.
- Opt-in MCP server for agent control.
- FFmpeg is looked up next to the executable before `PATH`, so release archives can ship it.

[0.3.0]: https://github.com/elisalien/verso/releases/tag/v0.3.0
