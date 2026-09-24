# Ferese

Ferese is an experimental Wayland tiling compositor written in Rust with
Smithay. Development currently targets the Phase 0 nested compositor described
in [spec.md](spec.md).

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

