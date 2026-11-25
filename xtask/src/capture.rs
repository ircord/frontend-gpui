//! `cargo xtask capture`: run the Linux binary against a disposable backend
//! on a virtual display and save screenshots, so a change can be looked at
//! (and checked) without a screen.
//!
//! In the build container it starts a session bus with GNOME Keyring, signs
//! up an account (with an IRC network and a channel) over HTTP, signs in
//! through the app's own form, captures the workspace, checks the sign-in is
//! in the keyring, types a server emoji into the message box, plays a test
//! video sent to the channel (printing the app's resident memory 2 s and
//! 32 s after it starts), sends `/attract` and captures the comet five
//! times while it plays (the last two around its landing) and once after it
//! ends, then restarts the app to show it signs in from there, and opens Settings → Emotes → Discover (emoji.gg's emoji, read by
//! the backend over the internet).
//! `CAPTURE_ONLY=fade` captures a change of colours fading, nothing else.
//! A personal theme's background picture is captured last, behind the window
//! and behind the conversation, in dark and light.
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
xdotool mousemove --window "$WIN" 600 435 click 1
sleep 0.5
xdotool type --delay 30 "$USER_NAME"
xdotool mousemove --window "$WIN" 600 506 click 1
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

# `CAPTURE_ONLY=fade`: a change of colours fades. The account's appearance is
# switched to dark while the app shows the workspace in light, and the screen
# is captured as fast as it goes (software drawing, so a few frames per
# second) until it has settled.
if [ "${CAPTURE_ONLY:-}" = fade ]; then
  sleep 2
  shot fade-0-before
  curl -fsS -o /dev/null -X PUT -H "Origin: $SERVER" -H "Cookie: $COOKIE" -H 'Content-Type: application/json' \
    -d '{"settings":{"mode":"dark"}}' "$SERVER/api/v1/app/appearance"
  for n in 1 2 3 4 5 6; do shot fade-$n-$(date +%s%3N); done
  sleep 2
  shot fade-9-after
  stop_app fade
  exit 0
fi
if [ -z "${CAPTURE_ONLY:-}" ]; then
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

# /attract: a comet laps the newest of your messages twice, shedding sparks
# over the rows below, and lands on its group's avatar; after three seconds
# it is gone. The first Return takes the suggested command, the second
# sends it.
for n in 1 2; do
  curl -fsS -o /dev/null -H "Origin: $SERVER" -H "Cookie: $COOKIE" -H 'Content-Type: application/json' \
    -d "{\"clientId\":\"capture-attract-$n\",\"text\":\"attract test $n\"}" \
    "$SERVER/api/v1/conversations/$CONVERSATION/messages"
done
sleep 3
xdotool windowfocus --sync "$WIN"
xdotool type --delay 30 "/attract"
sleep 1
xdotool key Return
sleep 0.5
xdotool key Return
sleep 0.5
shot 10-attract-comet-1
sleep 0.6
shot 10-attract-comet-2
sleep 0.9
shot 10-attract-comet-3
# The landing: the burst and the landing ring live from 2.6 to 3 s.
sleep 0.15
shot 10-attract-comet-4
sleep 0.1
shot 10-attract-comet-5
sleep 4
shot 10-attract-over

# A fresh start signs in from the keyring alone.
stop_app 1
start_app
sleep 7
shot 4-restarted

# Settings → Emotes → Discover, reached through the settings search: a page
# of emoji.gg's emoji with their pictures. The backend reads emoji.gg
# itself (this scene needs the internet), and a cold first read can fail,
# so ask until a page comes back first.
i=0
until curl -fsS -H "Origin: $SERVER" -H "Cookie: $COOKIE" "$SERVER/api/v1/emotes/discover?limit=30" \
  | grep -q '"providerId"'; do
  i=$((i + 1)); [ "$i" -lt 10 ] || { echo "Discover listed nothing (is emoji.gg reachable?)"; break; }
  sleep 10
done
xdotool windowfocus --sync "$WIN"
xdotool key ctrl+comma
sleep 1
xdotool key Tab
xdotool type --delay 30 "discover"
xdotool key Return
sleep 10
shot 9-emotes-discover
stop_app 2
else
stop_app 1
fi

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

# A personal theme with a background picture (the web's "Personal theme
# background"): the picture is uploaded, the theme names it, and the app
# draws it behind the whole window (`app`) or behind the conversation
# (`conversation`), in dark and in light. `CAPTURE_THEME=<dir>` swaps in a
# theme file (`theme.json`, an exported `.ircord-theme.json`) and its
# picture (`picture.*`) for the generated ones.
set -- $(curl -fsS -H "Origin: $SERVER" -H "Cookie: $COOKIE" "$SERVER/api/v1/bootstrap" | python3 -c '
import json, sys
s = json.load(sys.stdin)
c = [c for c in s["conversations"] if c["name"].lstrip("#").lower() == "capture"][0]
print(c["id"], c["networkId"])')
CONVERSATION=$1 NETWORK=$2
for n in 1 2 3; do
  curl -fsS -o /dev/null -H "Origin: $SERVER" -H "Cookie: $COOKIE" -H 'Content-Type: application/json' \
    -d "{\"clientId\":\"capture-backdrop-$n\",\"text\":\"a message to read over the picture, number $n\"}" \
    "$SERVER/api/v1/conversations/$CONVERSATION/messages"
done
if [ -n "${CAPTURE_THEME:-}" ]; then
  PICTURE=$(ls /theme/picture.* | head -1)
else
  PICTURE=/tmp/backdrop.png
  python3 -c '
import struct, sys, zlib
w, h = 640, 360
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
rows = b"".join(b"\0" + b"".join(
    bytes((40 + 150 * x // w, 60 + 100 * y // h, 200 - 120 * x // w)) for x in range(w)) for y in range(h))
sys.stdout.buffer.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
' >"$PICTURE"
fi
TYPE=$(python3 -c 'import sys; print({"png":"image/png","webp":"image/webp","jpg":"image/jpeg","jpeg":"image/jpeg"}[sys.argv[1].rsplit(".",1)[1]])' "$PICTURE")
PICTURE_URL=$(curl -fsS -H "Origin: $SERVER" -H "Cookie: $COOKIE" \
  -F "file=@$PICTURE;type=$TYPE" "$SERVER/api/v1/uploads?networkId=$NETWORK" \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["url"])')
theme() {
  python3 - "$1" "$PICTURE_URL" <<'PY' >/tmp/backdrop-theme.json
import json, os, sys
placement, url = sys.argv[1], sys.argv[2]
if os.environ.get("CAPTURE_THEME"):
    file = json.load(open("/theme/theme.json"))
    theme = {"format": 1, "id": "t-capture-bg-" + placement, "name": file["name"], "updatedAt": "c1",
             "base": file["base"], "colors": file["colors"], "css": file.get("css", ""),
             "background": dict(file.get("background") or {}, url=url)}
else:
    seeds = lambda c, s, o, i, m, a: {"canvas": c, "surface": s, "overlay": o, "ink": i, "muted": m, "accent": a,
        "palette": {k: v for k, v in zip(["red", "orange", "yellow", "green", "cyan", "blue", "purple", "pink"],
            ["#f87171", "#fb923c", "#fbbf24", "#22c55e", "#5fc4dc", "#86b6ef", "#b3a2ea", "#ee9fc4"])}}
    theme = {"format": 1, "id": "t-capture-bg-" + placement, "name": "Capture backdrop", "updatedAt": "c1",
             "base": {"dark": "ircord-dark", "light": "ircord-light"},
             "colors": {"dark": seeds("#07090d", "#161a22", "#2a3140", "#e8ebf1", "#9aa3b2", "#7ea6ff"),
                        "light": seeds("#e4e8ef", "#ffffff", "#d3d9e4", "#141821", "#5a6473", "#2b59c3")},
             "css": "",
             "background": {"url": url, "placement": placement, "fit": "cover", "position": "center",
                            "dim": 55, "blur": 0, "panelOpacity": 85}}
json.dump({"theme": theme}, sys.stdout)
PY
  curl -fsS -o /dev/null -X PUT -H "Origin: $SERVER" -H "Cookie: $COOKIE" -H 'Content-Type: application/json' \
    -d @/tmp/backdrop-theme.json "$SERVER/api/v1/app/themes/t-capture-bg-$1"
}
appearance() { api PUT "{\"settings\":{\"mode\":\"$1\",\"theme\":\"t-capture-bg-$2\"}}" /api/v1/app/appearance; }
theme app
for mode in dark light; do
  appearance $mode app
  start_app
  sleep 8
  shot 11-backdrop-app-$mode
  stop_app backdrop-app-$mode
done
if [ -z "${CAPTURE_THEME:-}" ]; then
  theme conversation
  appearance dark conversation
  start_app
  sleep 8
  shot 11-backdrop-conversation-dark
  stop_app backdrop-conversation
fi

# The theme editor's Background part: Settings → Appearance → Your themes →
# Edit, on the theme that has the picture.
appearance dark app
start_app
sleep 8
xdotool windowfocus --sync "$WIN"
xdotool key ctrl+comma
sleep 1
xdotool key Tab
xdotool type --delay 30 "your themes"
xdotool key Return
sleep 2
shot 12-theme-editor-0
# Edit on the second theme's row.
xdotool mousemove 939 506 click 1
sleep 2
xdotool mousemove 850 450
for n in 1 2 3; do
  xdotool click --repeat 5 --delay 100 5
  sleep 1
  shot 12-theme-editor-$n
done
stop_app theme-editor
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
    // `CAPTURE_ONLY=theme` skips the long scenes; `CAPTURE_THEME=<dir>` holds
    // a `theme.json` and a `picture.*` for the background scene.
    if let Ok(only) = std::env::var("CAPTURE_ONLY") {
        extra.extend(["-e".into(), format!("CAPTURE_ONLY={only}")]);
    }
    if let Ok(dir) = std::env::var("CAPTURE_THEME") {
        let dir = std::fs::canonicalize(&dir).with_context(|| format!("CAPTURE_THEME {dir}"))?;
        extra.extend([
            "-v".into(),
            format!("{}:/theme:ro", dir.display()),
            "-e".into(),
            "CAPTURE_THEME=1".into(),
        ]);
    }
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
