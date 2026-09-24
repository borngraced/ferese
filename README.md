# Ferese

Ferese is an experimental Wayland tiling compositor written in Rust with
Smithay. The nested compositor is complete and development currently targets
the M2 tiling and workspaces milestone in the
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
