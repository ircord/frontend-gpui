//! `cargo xtask capture`: run the Linux binary against a disposable backend
//! on a virtual display and save screenshots, so a change can be looked at
//! (and checked) without a screen.
//!
//! In the build container it starts a session bus with GNOME Keyring, signs
//! up an account (with an IRC network and a channel) over HTTP, signs in
//! through the app's own form, captures the workspace, checks the sign-in is
//! in the keyring, types a server emoji into the message box, plays a test
//! video sent to the channel (printing the app's resident memory 2 s and
//! 32 s after it starts), then restarts the app to show it signs in from
//! there.
//! Screenshots and the app's logs land in `target/capture/`; look at them.
//!
//! Xvfb runs without its COMPOSITE extension. With it, GPUI picks a 32-bit
//! ARGB visual for its transparent-capable window and, with no compositor
//! running, the software Vulkan driver presents it black. That was the
//! "black screenshot" of the first plan. A real desktop has a compositor.

use anyhow::{Context, Result};
use std::path::Path;

use crate::integration::Backend;

const SCRIPT: &str = r##"
set -eu
OUT=/work/target/capture
rm -rf "$OUT"; mkdir -p "$OUT"
export XDG_RUNTIME_DIR=/tmp/xdg XDG_CONFIG_HOME=/tmp/config XDG_DATA_HOME=/tmp/data
mkdir -p -m 700 "$XDG_RUNTIME_DIR"
SERVER=http://127.0.0.1:8080
USER_NAME="capture$(date +%s)"
PASSWORD="capture-$(od -An -N8 -tx8 /dev/urandom | tr -d ' ')"

i=0
until curl -fsS -o /dev/null -H "Origin: $SERVER" "$SERVER/api/v1/auth/session"; do
  i=$((i + 1)); [ "$i" -lt 120 ] || { echo "the backend never answered"; exit 1; }
  sleep 0.5
done
COOKIE=$(curl -fsS -D - -o /dev/null -H "Origin: $SERVER" -H 'Content-Type: application/json' \
  -d "{\"username\":\"$USER_NAME\",\"password\":\"$PASSWORD\"}" "$SERVER/api/v1/auth/signup" \
  | sed -n 's/^[Ss]et-[Cc]ookie: \(ircord_session=[^;]*\).*/\1/p')
[ -n "$COOKIE" ] || { echo "signup set no session"; exit 1; }
curl -fsS -o /dev/null -H "Origin: $SERVER" -H "Cookie: $COOKIE" -H 'Content-Type: application/json' \
  -d "{\"name\":\"Test\",\"host\":\"$IRC_HOST\",\"port\":6667,\"tls\":false,\"nick\":\"$USER_NAME\",\"autoJoin\":[\"#capture\"]}" \
  "$SERVER/api/v1/networks"

# A server emote, a 64x64 orange disc, for the message box to draw.
python3 -c '
import struct, sys, zlib
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
rows = b"".join(b"\0" + b"".join(
    bytes((240, 140, 40, 255) if (x - 31.5) ** 2 + (y - 31.5) ** 2 < 900 else (0, 0, 0, 0))
    for x in range(64)) for y in range(64))
sys.stdout.buffer.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 64, 64, 8, 6, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
' >/tmp/capture.png
curl -fsS -o /dev/null -H "Origin: $SERVER" -H "Cookie: $COOKIE" \
  -F name=Capture -F shortcode=capture -F "file=@/tmp/capture.png;type=image/png" "$SERVER/api/v1/emotes"

# An unlocked login keyring, as a desktop session has after signing in.
printf capture | gnome-keyring-daemon --unlock --components=secrets >/dev/null
gnome-keyring-daemon --start --components=secrets >/dev/null
printf probe | secret-tool store --label=probe capture probe
secret-tool lookup capture probe >/dev/null || { echo "the keyring does not work"; exit 1; }
secret-tool clear capture probe

Xvfb :99 -screen 0 1280x800x24 -extension COMPOSITE >/tmp/xvfb.log 2>&1 &
export DISPLAY=:99
i=0; until xdpyinfo >/dev/null 2>&1; do i=$((i + 1)); [ "$i" -lt 50 ] || exit 1; sleep 0.1; done

start_app() {
  IRCORD_SERVER=$SERVER RUST_LOG=${RUST_LOG:-info} dist/ircord >>"$OUT/stderr.log" 2>&1 &
  APP=$!
  i=0; until xdotool search --name '^IRCord$' >/dev/null 2>&1; do
    i=$((i + 1)); [ "$i" -lt 100 ] || { echo "no window"; exit 1; }; sleep 0.2
  done
  WIN=$(xdotool search --name '^IRCord$' | head -1)
  sleep 3
}
shot() { import -window root "$OUT/$1.png"; echo "captured target/capture/$1.png"; }
# The app logs to its settings folder, a fresh file each start.
stop_app() {
  kill "$APP"; wait "$APP" 2>/dev/null || true
  cp "$XDG_CONFIG_HOME/com.ircord.gpui/ircord.log" "$OUT/ircord-$1.log"
}

start_app
shot 1-sign-in
# No window manager runs, so give the window the keyboard focus directly.
# The window is 1200x800 at the screen's centre; the form is centred in it.
xdotool windowfocus --sync "$WIN"
xdotool mousemove --window "$WIN" 600 380 click 1
sleep 0.5
xdotool type --delay 30 "$USER_NAME"
xdotool mousemove --window "$WIN" 600 447 click 1
sleep 0.5
xdotool type --delay 30 "$PASSWORD"
shot 2-typed
xdotool key Return
sleep 10
shot 3-workspace
# The sign-in is in the keyring (attributes only; the secret is not shown).
secret-tool search --all service com.ircord.gpui 2>&1 | grep -v '^secret' >"$OUT/keyring.txt" || true
grep -q 'attribute.service = com.ircord.gpui' "$OUT/keyring.txt" \
  || { echo "the sign-in was not saved in the keyring"; exit 1; }
echo "the sign-in is in GNOME Keyring (target/capture/keyring.txt)"

# A server emoji typed into the message box draws as its picture. Typing
# with nothing focused goes into the box.
xdotool type --delay 30 "hello :capture: there"
sleep 2
shot 5-emoji-in-box
xdotool key ctrl+a BackSpace

# A video in a message plays in place (through the ircord-video helper
# beside the app): a 6 s test video (colour bars and a tone), uploaded and
# sent to #capture over HTTP, then played by clicking its play button.
gst-launch-1.0 -q videotestsrc num-buffers=180 ! video/x-raw,width=1280,height=720,framerate=30/1 \
  ! vp8enc deadline=1 ! webmmux name=m ! filesink location=/tmp/capture.webm \
  audiotestsrc num-buffers=260 ! audioconvert ! vorbisenc ! m.
set -- $(curl -fsS -H "Origin: $SERVER" -H "Cookie: $COOKIE" "$SERVER/api/v1/bootstrap" | python3 -c '
import json, sys
s = json.load(sys.stdin)
c = [c for c in s["conversations"] if c["name"].lstrip("#").lower() == "capture"][0]
print(c["id"], c["networkId"])')
CONVERSATION=$1 NETWORK=$2
set -- $(curl -fsS -H "Origin: $SERVER" -H "Cookie: $COOKIE" \
  -F "file=@/tmp/capture.webm;type=video/webm" "$SERVER/api/v1/uploads?networkId=$NETWORK" \
  | python3 -c 'import json, sys; u = json.load(sys.stdin); print(u["id"], u["url"])')
UPLOAD=$1 VIDEO_URL=$2
curl -fsS -o /dev/null -H "Origin: $SERVER" -H "Cookie: $COOKIE" -H 'Content-Type: application/json' \
  -d "{\"clientId\":\"capture-video\",\"text\":\"$VIDEO_URL\",\"attachments\":[\"$UPLOAD\"]}" \
  "$SERVER/api/v1/conversations/$CONVERSATION/messages"
sleep 3
shot 6-video-idle
# The play button: the middle of the newest message's 400x225 box, in
# the window (which starts 40 px right of the screen's left edge).
xdotool mousemove --window "$WIN" ${VIDEO_X:-520} ${VIDEO_Y:-555} click 1
sleep 2
shot 7-video-playing
rss() { ps -o rss= -p "$1" 2>/dev/null | tr -d ' '; }
HELPER=$(ps -eo pid=,args= | awk '$2 ~ /ircord-video$/ {print $1}' | tail -1)
ps -eo pid,rss,args | grep -v grep | grep -E "dist/ircord" | cut -c1-200 >"$OUT/video-ps.txt" || true
echo "resident memory at 2 s: app $(rss "$APP") kB, helper $(rss "$HELPER") kB" | tee "$OUT/video-memory.txt"
sleep 30
echo "resident memory at 32 s: app $(rss "$APP") kB, helper $(rss "$HELPER") kB" | tee -a "$OUT/video-memory.txt"
shot 8-video-ended

# A fresh start signs in from the keyring alone.
stop_app 1
start_app
sleep 7
shot 4-restarted
stop_app 2

# A personal theme whose CSS sets the accent and the rail, switched on for
# the account, then a fresh start: the app draws the theme's CSS.
THEME='{"format":1,"id":"t-capture-css","name":"Capture CSS","updatedAt":"c1","base":{"dark":"ircord-dark","light":"ircord-light"},"colors":{},"css":":root { --accent: #e0457b; --rail: #2a1f3d } .x { color: red }"}'
api() { curl -fsS -o /dev/null -X "$1" -H "Origin: $SERVER" -H "Cookie: $COOKIE" -H 'Content-Type: application/json' -d "$2" "$SERVER$3"; }
api PUT "{\"theme\":$THEME}" /api/v1/app/themes/t-capture-css
api PATCH '{"customCss":true}' /api/v1/app/themes
api PUT '{"settings":{"mode":"dark","theme":"t-capture-css"}}' /api/v1/app/appearance
start_app
sleep 7
shot 6-css-theme
stop_app 3
"##;

pub fn run(root: &Path) -> Result<()> {
    crate::container::build_linux(root)?;
    // The session bus refuses to start for a user ID with no name, and the
    // container runs as the host's user, which the image does not know.
    let passwd = root.join("target/capture-passwd");
    let (uid, gid) = (id("-u")?, id("-g")?);
    std::fs::write(
        &passwd,
        format!("root:x:0:0::/root:/bin/sh\nircord:x:{uid}:{gid}::/tmp:/bin/sh\n"),
    )?;
    let backend = Backend::start(root)?;
    let mut extra = backend.network_args();
    extra.extend([
        "-v".into(),
        format!("{}:/etc/passwd:ro", passwd.display()),
        "-e".into(),
        format!("IRC_HOST={}", backend.ergo),
        "--shm-size".into(),
        "256m".into(),
    ]);
    let result = crate::container::run_with(
        root,
        &extra,
        &["dbus-run-session", "--", "sh", "-c", SCRIPT, "capture"],
    );
    if result.is_err() {
        backend.print_logs();
    }
    drop(backend);
    result.context("the capture run failed")?;
    println!("screenshots and the app's logs are in target/capture/");
    Ok(())
}

fn id(flag: &str) -> Result<String> {
    let out = std::process::Command::new("id").arg(flag).output()?;
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}
