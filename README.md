# Verso

Free and open source VJ software. Clip grid of layers × columns, GPU compositing
with blend modes, live output window. Written in Rust on wgpu, for Windows, macOS
and Linux.

> Status: early v0.1. Generative sources only; video playback arrives in v0.2.
> See [docs/SPEC.md](docs/SPEC.md) (French) for the architecture and roadmap.

## Run

```bash
cargo run --release -p verso-app
```

## Controls

| Input | Action |
|---|---|
| Click a cell | Trigger that clip on its layer (empty cell ejects the layer) |
| Click a column header, or keys `1`–`9` | Trigger the whole column |
| Right-click a cell | Change its source or clear it |
| `Esc` | Clear all layers |
| `F2` | Toggle the output window (double-click it for fullscreen) |

## Workspace

| Crate | Role |
|---|---|
| `verso-core` | Composition model and commands, no GPU dependency |
| `verso-render` | wgpu engine: sources, blend modes, master |
| `verso-app` | eframe/egui application |

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
