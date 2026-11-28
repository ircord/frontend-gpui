# The `CAPTURE_ONLY=e2ee-group` scene of `cargo xtask capture`, sourced by
# its script once the app has signed in (`$WIN`, `$SERVER`, `$USER_NAME`,
# `$PASSWORD`, `shot`, `start_app` and `stop_app` come from there). Two
# headless members (`crates/client/examples/e2ee-group.rs`): Alex on this
# server starts the group "Weekend plans" with this account and Sam, whose
# IRCord server is the second backend on the same IRC network
# (`$SERVER_B`). Click positions are in the 1200x800 window; `E2EE_*`
# overrides them.

await() {
  i=0
  until [ -e "$1" ]; do
    i=$((i + 1)); [ "$i" -lt 900 ] || { echo "a member never got to $1"; cat "$OUT"/group-*.log; exit 1; }
    sleep 0.2
  done
}
click() { xdotool windowraise "$1"; xdotool mousemove --window "$1" "$2" "$3" click 1; sleep "${4:-1}"; }
win_shot() { xdotool windowraise "$1"; sleep 0.5; import -window "$1" "$OUT/$2.png"; echo "captured target/capture/$2.png"; }

SERVER_B=${SERVER_B:-http://127.0.0.1:8082}
ALEX="alex$(date +%s)"
SAM="sam$(date +%s)"
cargo build -q -p ircord-client --example e2ee-group
# Sam is on the other server.
target/debug/examples/e2ee-group "$SERVER_B" "$SAM" "sam-$PASSWORD" /tmp/sam join >"$OUT/group-sam.log" 2>&1 &
SAM_PID=$!
target/debug/examples/e2ee-group "$SERVER" "$ALEX" "alex-$PASSWORD" /tmp/alex create "Weekend plans" "$USER_NAME" "$SAM" >"$OUT/group-alex.log" 2>&1 &
ALEX_PID=$!
await /tmp/sam/ready
await /tmp/alex/ready
sleep 4
# The first sign-in asks for a profile; not now.
click "$WIN" "${E2EE_SKIP_X:-668}" "${E2EE_SKIP_Y:-620}" 1
# This device set up, then the group starts.
sleep 8
touch /tmp/alex/go
await /tmp/alex/created
# The Welcome travels over IRC; the app reads it at its next sync.
sleep 35
win_shot "$WIN" e2ee-group-1-invitation

# Join from the invitation card.
click "$WIN" "${E2EE_JOIN_X:-1153}" "${E2EE_JOIN_Y:-74}" 8
await /tmp/sam/joined
win_shot "$WIN" e2ee-group-2-joined

# The group, listed among the channels by its title.
click "$WIN" "${E2EE_GROUP_X:-80}" "${E2EE_GROUP_Y:-341}" 3
echo "Who is in for Saturday?" >/tmp/alex/say
await /tmp/alex/said
sleep 3
echo "Count me in, from the other server." >/tmp/sam/say
await /tmp/sam/said
sleep 5
win_shot "$WIN" e2ee-group-3-conversation

# The lock in the header opens the group's panel.
click "$WIN" "${E2EE_LOCK_X:-1020}" "${E2EE_LOCK_Y:-70}" 2
win_shot "$WIN" e2ee-group-4-panel
xdotool mousemove --window "$WIN" 1090 500
xdotool click --repeat 8 --delay 80 5
sleep 1
win_shot "$WIN" e2ee-group-5-members
xdotool click --repeat 8 --delay 80 4

# A message from here.
xdotool windowfocus --sync "$WIN"
click "$WIN" "${E2EE_BOX_X:-450}" "${E2EE_BOX_Y:-765}" 0.5
xdotool type --delay 20 "Me too. Only the three of us can read this."
xdotool key Return
sleep 6
win_shot "$WIN" e2ee-group-6-messages

kill "$ALEX_PID" "$SAM_PID" 2>/dev/null || true
stop_app e2ee-group
