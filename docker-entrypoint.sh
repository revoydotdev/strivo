#!/bin/sh
# Entrypoint for the strivo image.
#
# Responsibilities, in order:
#   1. Make sure the mounted volumes exist and are owned by the `strivo`
#      user, then drop root.
#   2. Seed a minimal config.toml on first run so `recording_dir` points at
#      the /recordings volume instead of strivo's normal default of
#      ~/Videos/StriVo (see default_recording_dir() in src/config/mod.rs) —
#      without this the container would record into an unmounted, disposable
#      layer.
#   3. Start a headless D-Bus session bus + gnome-keyring so strivo's OS
#      keyring credential storage (src/config/credentials.rs — OAuth
#      access/refresh tokens for Twitch/YouTube/Patreon) has a Secret
#      Service to talk to. A container has no login session to unlock a
#      real one, so this creates/unlocks a keyring with an empty password.
#      This is a real Secret Service, not a stub — `keyring::Entry::get/set
#      password` round-trips through it — but the keyring file lives on the
#      /config volume with no login-session protection, so treat it as
#      lightly-obfuscated storage, not a security boundary. See
#      docs/DOCKER.md "Credentials" for the full story and the alternative
#      of injecting tokens via a secrets manager.
#   4. exec the real command as the strivo user.
set -eu

RECORDINGS_DIR=/recordings
CONFIG_DIR=${XDG_CONFIG_HOME:-/config}/strivo

mkdir -p "$RECORDINGS_DIR" "$CONFIG_DIR" "${XDG_DATA_HOME:-/config/data}" "${XDG_STATE_HOME:-/config/state}"
chown -R strivo:strivo /recordings /config /home/strivo

if [ ! -f "$CONFIG_DIR/config.toml" ]; then
    cat > "$CONFIG_DIR/config.toml" <<EOF
recording_dir = "$RECORDINGS_DIR"
EOF
    chown strivo:strivo "$CONFIG_DIR/config.toml"
fi

# Headless D-Bus session bus + gnome-keyring, unlocked with an empty
# passphrase. Runs as the strivo user so the keyring file under
# /home/strivo/.local/share/keyrings is owned correctly.
export $(dbus-launch)
KEYRING_OUT=$(gosu strivo sh -c 'printf "" | gnome-keyring-daemon --start --components=secrets --unlock' 2>/dev/null || true)
eval "$KEYRING_OUT"
export GNOME_KEYRING_CONTROL

exec gosu strivo env DBUS_SESSION_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" GNOME_KEYRING_CONTROL="${GNOME_KEYRING_CONTROL:-}" "$@"
