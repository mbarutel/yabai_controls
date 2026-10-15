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

## Dedicating spaces to apps

```toml
[spaces]
1 = { app = "Google Chrome" }                     # label defaults to the app name
2 = { app = "Microsoft Teams", label = "teams" }
3 = { app = "Ghostty", follow = true }            # focus the space on new windows
4 = { label = "scratch" }                         # label only
```

On startup and `reload` the daemon creates any missing spaces, sets each
label, and adds a yabai rule `app="^<app>$" space=<index>` labelled
`yabai_controls_space_<index>`, then applies it so already-open windows move.
Creating spaces needs yabai's scripting addition (`sudo yabai --load-sa`);
without it, assignments for missing spaces are skipped with a warning and the
daemon still starts. When a space later appears (yabai's `space_created`
signal, so not with `--no-signals`), skipped assignments whose space now
exists are labelled and their rule installed, moving the app's open windows;
rules already installed are not re-applied. Any `yabai_controls_space_*` rule in yabai that the
config no longer wants (including leftovers from a run that did not shut down
cleanly) is removed. On shutdown the rules are removed; labels stay (yabai has
no way to clear one). Bindings can target the labels:
`"alt - t" = "space --focus Ghostty"`.

## Focus trail

The daemon keeps a back/forward history of focused windows and of focused
spaces, fed by yabai's `window_focused` and `space_changed` signals (so it
needs signals, i.e. not `--no-signals`). Bind `{ focus = "previous-window" }`
/ `"next-window"` / `"previous-space"` / `"next-space"` to walk it. Like a
browser: stepping back and then focusing something else by hand discards the
forward part. Spaces are tracked by id, so the trail survives index shifts
when spaces are created or destroyed. The trail starts empty at launch (seeded
with the current focus), holds 32 entries, and forgets closed windows.

## Layout

```
src/
  lib.rs          module map + thread diagram
  main.rs         CLI (daemon | notify | reload | shutdown | state | check-config)
  config.rs       TOML -> validated bindings per mode + space assignments
  spaces.rs       plan/apply [spaces]: create, label, app rules
  keys/           Hotkey, chord parser, kVK keycode table, CGEventTap
  yabai/          socket client, serde models, typed queries, signal registration
  state.rs        in-memory displays/spaces/windows snapshot
  events.rs       Event enum + the daemon's own control socket
  os_signal.rs    SIGINT/SIGTERM → graceful shutdown (second one force-exits)
  actions.rs      Action enum + worker thread
  focus.rs        back/forward focus trail (previous/next window and space)
  dispatch.rs     the event loop
tests/protocol.rs wire-protocol round-trip against a fake daemon
```
