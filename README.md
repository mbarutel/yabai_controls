# yabai_controls

A hotkey daemon for [yabai](https://github.com/koekeishiya/yabai): listens for
key chords with a CGEventTap and sends commands straight to yabai's socket.

## Run

Try it from a terminal first:

```sh
cargo build
mkdir -p ~/.config/yabai_controls
cp config.example.toml ~/.config/yabai_controls/config.toml
# stop skhd first if it binds the same chords
skhd --stop-service
RUST_LOG=info target/debug/yabai_controls daemon
```

The first run needs the Accessibility permission: System Settings → Privacy &
Security → Accessibility → add the binary. The Rust linker ad-hoc signs every
build, and macOS ties the grant to that signature, so **every rebuild
invalidates the grant**: remove the stale entry and add the binary again (or
toggle it off and on). To make the grant survive rebuilds, create a
self-signed code-signing certificate once (Keychain Access → Certificate
Assistant → Create a Certificate, type "Code Signing", e.g. named
`yabai_controls`) and sign each build with it before granting:

```sh
codesign -f -s yabai_controls ~/.cargo/bin/yabai_controls
```

### Install as a launch agent

```sh
./install.sh
```

The script builds a release binary, validates `config.toml` with it (aborts
before touching anything if the config is bad), `cargo install`s to
`~/.cargo/bin/yabai_controls`, signs it if a `yabai_controls` code-signing
identity exists (`SIGN_IDENTITY=…` to use another name), copies
`launchd/com.asmvik.yabai_controls.plist` to `~/Library/LaunchAgents/`,
restarts the agent (`launchctl bootout` + `bootstrap`, so plist changes are
picked up) and reports whether the daemon came up. Re-run it after every
change. If the daemon fails with `failed to create event tap`, the script
opens the Accessibility pane: re-add `~/.cargo/bin/yabai_controls` there
(unless you signed it, see above).

By hand, the same steps are:

```sh
cargo install --path .                       # ~/.cargo/bin/yabai_controls
cp launchd/com.asmvik.yabai_controls.plist ~/Library/LaunchAgents/
# grant Accessibility to ~/.cargo/bin/yabai_controls, then:
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.asmvik.yabai_controls.plist
```

Logs go to `/tmp/yabai_controls_$USER.{out,err}.log`. `KeepAlive` restarts the
daemon after a crash or when it exits because yabai was not up yet;
`yabai_controls shutdown` keeps it down until the next login or
`launchctl kickstart gui/$(id -u)/com.asmvik.yabai_controls`. Until the
Accessibility grant is in place launchd keeps retrying the daemon every few
seconds and the err log fills with `failed to create event tap`; it starts
working as soon as the grant lands.

yabai keeps the rules and signals this daemon installs only in memory, so a
yabai restart silently drops them. Add this as the last line of `.yabairc`;
`reload` re-registers the signals and re-syncs `[spaces]`:

```sh
yabai_controls reload 2>/dev/null || true
```

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
2 = { apps = ["Microsoft Teams", "Microsoft Outlook"], label = "comms" }
3 = { app = "Ghostty", follow = true }            # focus the space on new windows
4 = { label = "scratch" }                         # label only
```

A space takes one `app` or several `apps` (then `label` is required); an app
can only belong to one space. On startup and `reload` the daemon creates any
missing spaces, sets each label, and adds one yabai rule per space,
`app="^(<app>|<app>)$" space=<index>` labelled `yabai_controls_space_<index>`,
then applies it so already-open windows move.
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

## Previous window / space

The daemon remembers the current and the previously focused window, and the
current and previously focused space, fed by yabai's `window_focused` and
`space_changed` signals (so it needs signals, i.e. not `--no-signals`). Bind
`{ focus = "previous-window" }` / `"previous-space"` to jump back; the two
slots swap, so pressing again returns you, alt-tab style. There is no
forward. The two are independent: clicking between windows on one space does
not change which space is "previous". Spaces are tracked by id, so the memory
survives index shifts when spaces are created or destroyed. Closed windows and
destroyed spaces are forgotten; if the current window closes, "previous" still
points to where you were before it.

## Clockwise / counter-clockwise focus

yabai's `window --focus east|west|…` selectors are geometric, so they do
nothing useful in a stack (every window has the same frame). Bind
`{ focus = "clockwise" }` / `"counter-clockwise"` to cycle every window on the
focused space regardless of layout. The ring is computed fresh from yabai on
each press: windows sharing a frame (a stack) form one slot, slots are ordered
by their angle around the centroid of all slots (clockwise on screen), a stack
is walked in place by stack index before moving on, floating windows are
included, minimized/hidden windows and dialogs are skipped, and a window
sitting on the centroid (e.g. the middle of a 3×3 grid) comes last.

## Jump to an app

`"alt - c" = { app = "Google Chrome" }` focuses a Chrome window wherever it
is; yabai switches space and display as needed. The name must match yabai's
`app` field exactly (`yabai -m query --windows | jq -r .[].app`). Which
window: the one you focused most recently, unless a Chrome window is already
focused, in which case the next Chrome window by id (wrapping), so repeated
presses walk all its windows across spaces. Minimized/hidden windows and
dialogs are skipped. With `launch = true` the binding runs `open -a <app>`
when the app has no window, which also brings forward a running app with
none. The choice comes from the daemon's snapshot, which yabai's window
signals keep current; recency is fed by `window_focused`, so it needs signals
(not `--no-signals`).

## Layout

```
src/
  lib.rs          module map + thread diagram
  main.rs         CLI (daemon | notify | reload | shutdown | state | check-config)
  config.rs       TOML -> validated bindings per mode + space assignments
  spaces.rs       plan/apply [spaces]: create, label, app rules
  keys/           Hotkey, chord parser, kVK keycode table, CGEventTap
  yabai/          socket client, serde models, typed queries, signal registration
  state.rs        in-memory displays/spaces/windows snapshot + `state` dump
  events.rs       Event enum + the daemon's own control socket
  os_signal.rs    SIGINT/SIGTERM → graceful shutdown (second one force-exits)
  actions.rs      Action (binding) / Command (worker) enums + worker thread
  focus.rs        current/previous focus memory (previous window and space)
  ring.rs         clockwise ordering of a space's windows (any layout)
  dispatch.rs     the event loop
tests/protocol.rs wire-protocol round-trip against a fake daemon
```
