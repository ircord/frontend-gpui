# The `CAPTURE_ONLY=e2ee` scene of `cargo xtask capture`, sourced by its
# script once the app has signed in (`$WIN`, `$SERVER`, `$USER_NAME`,
# `$PASSWORD`, `shot`, `start_app` and `stop_app` come from there). Click
# positions are in the 1200x800 window; `E2EE_*` overrides them.

# Wait for a file the peer writes, or give up.
await() {
  i=0
  until [ -e "$1" ]; do
    i=$((i + 1)); [ "$i" -lt 600 ] || { echo "the peer never got to $(basename "$1")"; cat "$OUT/peer.log"; exit 1; }
    sleep 0.2
  done
}
click() { xdotool windowraise "$1"; xdotool mousemove --window "$1" "$2" "$3" click 1; sleep "${4:-1}"; }
win_shot() { xdotool windowraise "$1"; sleep 0.5; import -window "$1" "$OUT/$2.png"; echo "captured target/capture/$2.png"; }
settings_encryption() {
  xdotool windowraise "$1"; xdotool windowfocus --sync "$1"
  xdotool key ctrl+comma; sleep 1
  xdotool key Tab; xdotool type --delay 30 "encryption"; xdotool key Return; sleep 2
}

PEER="peer$(date +%s)"
cargo build -q -p ircord-client --example e2ee-peer
target/debug/examples/e2ee-peer "$SERVER" "$PEER" "peer-$PASSWORD" "$USER_NAME" /tmp/peer >"$OUT/peer.log" 2>&1 &
PEER_PID=$!
await /tmp/peer/ready
sleep 4
# The first sign-in asks for a profile; not now.
click "$WIN" "${E2EE_SKIP_X:-668}" "${E2EE_SKIP_Y:-620}" 1
# Open this side of the direct conversation, as writing to them would.
curl -fsS -o /dev/null -H "Origin: $SERVER" -H "Cookie: $COOKIE" -H 'Content-Type: application/json' \
  -d "{\"networkId\":\"ircord\",\"nickname\":\"$PEER\"}" "$SERVER/api/v1/conversations"
sleep 2

# This device set up: Settings → Encryption.
settings_encryption "$WIN"
win_shot "$WIN" e2ee-1-set-up
xdotool key Escape; sleep 1

# The peer's direct message, opened with the switcher.
xdotool windowfocus --sync "$WIN"
xdotool key ctrl+k; sleep 1
xdotool type --delay 30 "$PEER"; sleep 1
xdotool key Return; sleep 3
win_shot "$WIN" e2ee-2-direct-message

# The lock in the conversation's header opens the panel; it starts the chat.
click "$WIN" "${E2EE_LOCK_X:-1088}" "${E2EE_LOCK_Y:-70}" 2
win_shot "$WIN" e2ee-3-panel-not-encrypted
click "$WIN" "${E2EE_START_X:-1090}" "${E2EE_START_Y:-415}" 2
await /tmp/peer/on
sleep 5
win_shot "$WIN" e2ee-4-encrypted
# Further down the panel: the security code, as digits and as a QR code.
xdotool mousemove --window "$WIN" 1090 500
xdotool click --repeat 8 --delay 80 5
sleep 1
win_shot "$WIN" e2ee-4b-security-code
xdotool click --repeat 8 --delay 80 4

# A message each way.
xdotool windowfocus --sync "$WIN"
click "$WIN" "${E2EE_BOX_X:-450}" "${E2EE_BOX_Y:-765}" 0.5
xdotool type --delay 20 "Nice, only our devices can read this now."
xdotool key Return
sleep 6
win_shot "$WIN" e2ee-5-messages

# A second device of this account: another copy of the app, signed in as
# `localhost` through its own form.
WIN1=$WIN
XDG_CONFIG_HOME=/tmp/config2 IRCORD_SERVER=http://localhost:8080 RUST_LOG=${RUST_LOG:-info} \
  "$APP_BIN" >>"$OUT/stderr-second.log" 2>&1 &
APP2=$!
i=0; until [ "$(xdotool search --name '^IRCord$' | wc -l)" -ge 2 ]; do
  i=$((i + 1)); [ "$i" -lt 100 ] || { echo "no second window"; exit 1; }; sleep 0.2
done
WIN2=$(xdotool search --name '^IRCord$' | grep -vx "$WIN1" | head -1)
sleep 3
xdotool windowraise "$WIN2"; xdotool windowfocus --sync "$WIN2"
xdotool mousemove --window "$WIN2" 600 435 click 1; sleep 0.5
xdotool type --delay 30 "$USER_NAME"
xdotool mousemove --window "$WIN2" 600 506 click 1; sleep 0.5
xdotool type --delay 30 "$PASSWORD"
xdotool key Return
sleep 10
settings_encryption "$WIN2"
win_shot "$WIN2" e2ee-6-new-device-waiting

# A third device of the account, headless, waits too and writes the code it
# shows (`crates/client/examples/e2ee-device.rs`); the first device counts
# both and approves that one with its code typed in.
cargo build -q -p ircord-client --example e2ee-device
target/debug/examples/e2ee-device "$SERVER" "$USER_NAME" "$PASSWORD" /tmp/third wait >"$OUT/third.log" 2>&1 &
THIRD_PID=$!
await /tmp/third/code
settings_encryption "$WIN1"
sleep 4
win_shot "$WIN1" e2ee-7-approve-form
click "$WIN1" "${E2EE_CODE_X:-660}" "${E2EE_CODE_Y:-363}" 0.5
xdotool type --delay 30 "$(cat /tmp/third/code)"
xdotool key Return
await /tmp/third/approved
sleep 3
win_shot "$WIN1" e2ee-8-approved
win_shot "$WIN2" e2ee-9-new-device-still-waiting

kill "$APP2" "$PEER_PID" "$THIRD_PID" 2>/dev/null || true
wait "$APP2" 2>/dev/null || true
cp /tmp/config2/com.ircord.gpui/ircord.log "$OUT/ircord-second.log" 2>/dev/null || true
stop_app e2ee
