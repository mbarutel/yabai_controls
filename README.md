# yabai_controls

A hotkey daemon for [yabai](https://github.com/koekeishiya/yabai): listens for
key chords with a CGEventTap and sends commands straight to yabai's socket.

## Run

```sh
cargo build --release
mkdir -p ~/.config/yabai_controls
cp config.example.toml ~/.config/yabai_controls/config.toml

# optional but recommended: ad-hoc sign so the Accessibility grant survives rebuilds
codesign -s - target/release/yabai_controls

# stop skhd first if it binds the same chords
skhd --stop-service

RUST_LOG=info target/release/yabai_controls daemon
```

The first run needs the Accessibility permission: System Settings → Privacy &
Security → Accessibility → add `target/release/yabai_controls`.

Other commands:

```sh
yabai_controls state          # dump displays/spaces/windows from yabai
yabai_controls check-config   # validate the config
yabai_controls reload         # hot-reload config in the running daemon
yabai_controls shutdown
```

`yabai_controls notify …` is what yabai calls from the signals the daemon
registers; you don't run it by hand.

## Layout

```
src/
  lib.rs          module map + thread diagram
  main.rs         CLI (daemon | notify | reload | shutdown | state | check-config)
  config.rs       TOML -> validated bindings per mode
  keys/           Hotkey, chord parser, kVK keycode table, CGEventTap
  yabai/          socket client, serde models, typed queries, signal registration
  state.rs        in-memory displays/spaces/windows snapshot
  events.rs       Event enum + the daemon's own control socket
  actions.rs      Action enum + worker thread
  dispatch.rs     the event loop
tests/protocol.rs wire-protocol round-trip against a fake daemon
```
