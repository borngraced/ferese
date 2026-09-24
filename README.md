# Ferese

Ferese is an experimental Wayland tiling compositor written in Rust with
Smithay. The nested M0–M3 path is complete, including tiling, workspaces, and
timestamp-driven animated geometry. Development now targets the M4 early
hardware-session milestone in the
[window-manager specification](docs/ferese-window-manager-spec.md).

## Development

Install the native dependencies required by Smithay's winit and GLES backends,
then run:

```bash
cargo run -p ferese
```

Pass a client and its arguments after `--` to launch it inside Ferese:

```bash
cargo run -p ferese -- foot
```

Set `RUST_LOG=ferese=debug` for detailed compositor logging.
