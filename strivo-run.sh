#!/bin/sh
# Default container command: run the daemon and the web UI as two
# supervised processes in one container, sharing the daemon's Unix socket
# under $XDG_STATE_HOME/strivo (see src/ipc.rs socket_path()/state_dir()).
#
# Why two processes instead of the bare `strivo` single-process default:
# `run_default_webui()` in crates/strivo-bin/src/main.rs hardcodes
# `strivo-web` to bind 127.0.0.1:8181, which is unreachable from outside
# the container even with `-p 8181:8181` published. `strivo serve --bind`
# accepts an explicit bind address, but `strivo serve` alone does not spawn
# a daemon — only the bare `strivo` entry point does that. So getting a
# routable port with the daemon running takes exactly the two processes
# this script supervises: `strivo daemon` in the background, `strivo serve
# --bind 0.0.0.0:8181` in the foreground. They rendezvous over the daemon
# Unix socket, not a shared filesystem path either process configures
# directly.
#
# Both are real child processes of this script (not double-forked), so
# SIGTERM delivered to this script — forwarded here by tini, itself PID 1
# — is relayed to both and we wait for a clean exit before returning,
# instead of leaving the daemon to be SIGKILLed uncleanly at the
# container's stop-timeout.
set -eu

STRIVO_BIND=${STRIVO_BIND:-0.0.0.0:8181}
SOCKET_PATH="${XDG_STATE_HOME:-/config/state}/strivo/strivo.sock"

strivo daemon &
daemon_pid=$!

# `strivo serve` connects to the daemon's IPC socket immediately and exits
# with "daemon not running" if it isn't there yet (unlike the bare `strivo`
# entry point, which polls for up to ~3s before serving — see
# run_default_webui() in crates/strivo-bin/src/main.rs). Reproduce that
# wait here so the two processes don't race on startup.
i=0
while [ ! -S "$SOCKET_PATH" ] && [ "$i" -lt 100 ]; do
    sleep 0.1
    i=$((i + 1))
done

strivo serve --bind "$STRIVO_BIND" &
serve_pid=$!

term() {
    kill -TERM "$daemon_pid" "$serve_pid" 2>/dev/null || true
}
trap term TERM INT

# `wait -n` (wait for whichever child exits first) is a bashism; this
# image's /bin/sh is dash. Poll instead — exit as soon as either process
# dies, since a live web UI with a dead daemon (or vice versa) is not a
# healthy container and restart policy should recycle the whole thing.
while kill -0 "$daemon_pid" 2>/dev/null && kill -0 "$serve_pid" 2>/dev/null; do
    sleep 1
done
term
wait || true
