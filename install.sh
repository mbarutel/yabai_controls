#!/usr/bin/env bash
# Build the working tree, install it to ~/.cargo/bin, (re)load the launch
# agent, and check that the daemon came up. Safe to re-run after every change.
#
#   ./install.sh
#
# Env overrides:
#   SIGN_IDENTITY  Keychain code-signing identity (default: yabai_controls).
#                  Used only if it exists; keeps the Accessibility grant across
#                  rebuilds. See README.
set -euo pipefail

cd "$(dirname "$0")"

LABEL=com.asmvik.yabai_controls
PLIST_SRC=launchd/$LABEL.plist
PLIST_DST=$HOME/Library/LaunchAgents/$LABEL.plist
BIN=$HOME/.cargo/bin/yabai_controls
DOMAIN=gui/$(id -u)
ERR_LOG=/tmp/yabai_controls_$USER.err.log
SIGN_IDENTITY=${SIGN_IDENTITY:-yabai_controls}

step() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

step "Building release binary"
cargo build --release

# Validate the config with the *new* binary before touching the running daemon,
# so a config the new version rejects doesn't take the old one down.
step "Checking config"
target/release/yabai_controls check-config \
    || die "fix ~/.config/yabai_controls/config.toml and re-run; nothing was installed"

step "Installing to $BIN"
# --target-dir reuses the build above instead of rebuilding in a temp dir.
cargo install --path . --target-dir target --quiet

if security find-identity -v -p codesigning | grep -q "\"$SIGN_IDENTITY\""; then
    step "Signing with '$SIGN_IDENTITY'"
    codesign -f -s "$SIGN_IDENTITY" "$BIN"
    signed=1
else
    signed=0
fi

step "Installing launch agent"
install -d "$HOME/Library/LaunchAgents"
cp "$PLIST_SRC" "$PLIST_DST"
plutil -lint "$PLIST_DST"

step "Restarting daemon"
# bootout sends SIGTERM (graceful shutdown) and forgets the old plist;
# bootstrap loads the new one and starts the daemon (RunAtLoad).
if launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; then
    launchctl bootout "$DOMAIN/$LABEL"
    for _ in $(seq 1 50); do
        launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1 || break
        sleep 0.2
    done
fi
launchctl bootstrap "$DOMAIN" "$PLIST_DST"

step "Verifying"
sleep 3
if pid=$(pgrep -f "$BIN daemon"); then
    echo "daemon running (pid $pid)"
    "$BIN" --version
    exit 0
fi

echo "daemon is not running; last log lines:"
tail -n 5 "$ERR_LOG" 2>/dev/null || true
if grep -q "failed to create event tap" "$ERR_LOG" 2>/dev/null; then
    cat <<EOF

Accessibility grant is missing or stale for this build.
  System Settings -> Privacy & Security -> Accessibility:
  remove any existing 'yabai_controls' entry, then add (Cmd-Shift-G):
    $BIN
launchd keeps retrying; the daemon starts working as soon as the grant lands.
EOF
    if [ "$signed" = 0 ]; then
        echo "Tip: create a self-signed '$SIGN_IDENTITY' code-signing certificate so the grant survives rebuilds (see README)."
    fi
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
fi
exit 1
