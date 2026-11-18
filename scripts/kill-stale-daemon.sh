#!/bin/sh
# Free /tmp/yabai_controls_$USER.socket when a leftover daemon holds it,
# typically one parked under a debugger (Zed/CodeLLDB) that ignores
# `yabai_controls shutdown` and plain `kill`.
set -eu

sock="/tmp/yabai_controls_${USER}.socket"

pids=$(lsof -t -U 2>/dev/null -a -c yabai_con || true)
if [ -z "$pids" ]; then
    echo "nothing holds $sock"
    exit 0
fi

for pid in $pids; do
    stat=$(ps -o stat= -p "$pid" | tr -d ' ')
    tracer=$(ps -o ppid= -p "$pid" | tr -d ' ')
    echo "daemon pid $pid (state $stat) holds $sock"
    case "$stat" in
        *X*|*T*)
            # Stopped under ptrace: kill the debugserver holding it, then the daemon.
            echo "  traced by pid $tracer ($(ps -o comm= -p "$tracer")); killing tracer"
            kill "$tracer" 2>/dev/null || true
            sleep 1
            kill -9 "$pid" 2>/dev/null || true
            ;;
        *)
            kill "$pid"
            ;;
    esac
done

sleep 1
if lsof -t -U 2>/dev/null -a -c yabai_con | grep -q .; then
    echo "still held:" >&2
    lsof -U | grep yabai_controls >&2
    exit 1
fi
echo "socket free"
