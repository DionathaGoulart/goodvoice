#!/usr/bin/env bash
#
# End-to-end check against a deployed goodvoice Worker.
#
#   bash server/scripts/smoke.sh https://goodvoice.<subdomain>.workers.dev
#
# Exercises health, join, the roster push over the WebSocket, and the 8-person
# cap. Needs curl and Node 22+ (for the global WebSocket); no other dependency.

set -euo pipefail

BASE="${1:-}"
if [[ -z "$BASE" ]]; then
  echo "usage: $0 <worker-url>" >&2
  exit 64
fi
BASE="${BASE%/}"

ROOM="smoke-$(date +%s)"
pass() { printf '  ok   %s\n' "$1"; }
fail() { printf '  FAIL %s\n' "$1" >&2; exit 1; }

echo "goodvoice smoke test against $BASE (room $ROOM)"

# --- health ------------------------------------------------------------------
health="$(curl -fsS "$BASE/health")"
[[ "$health" == '{"ok":true}' ]] || fail "health returned: $health"
pass "GET /health"

# --- join --------------------------------------------------------------------
join() {
  curl -sS -o /dev/stdout -w '\n%{http_code}' \
    -X POST "$BASE/rooms/$ROOM/join" \
    -H 'content-type: application/json' \
    -d "{\"name\":\"$1\"}"
}

first="$(join smoke-one)"
status="$(tail -n1 <<<"$first")"
body="$(sed '$d' <<<"$first")"
[[ "$status" == "200" ]] || fail "join returned $status: $body"

participant="$(node -e 'let d="";process.stdin.on("data",c=>d+=c).on("end",()=>{
  const b = JSON.parse(d);
  if (!b.self) { console.error("no self in join response"); process.exit(1); }
  if (!b.sfu?.sessionId) { console.error("no sfu.sessionId in join response"); process.exit(1); }
  if (!Array.isArray(b.sfu.iceServers) || b.sfu.iceServers.length === 0) {
    console.error("no iceServers in join response"); process.exit(1);
  }
  console.log(b.self);
})' <<<"$body")"
pass "POST /rooms/:code/join (session + ICE servers returned)"

# --- websocket ---------------------------------------------------------------
WS_BASE="${BASE/https:\/\//wss://}"
WS_BASE="${WS_BASE/http:\/\//ws://}"

BASE="$BASE" ROOM="$ROOM" WS_BASE="$WS_BASE" PARTICIPANT="$participant" node --input-type=module -e '
const { WS_BASE, BASE, ROOM, PARTICIPANT } = process.env;
const socket = new WebSocket(`${WS_BASE}/rooms/${ROOM}/ws?p=${PARTICIPANT}`);
const seen = [];

const done = (code, message) => { if (message) console[code ? "error" : "log"](message); process.exit(code); };
const timer = setTimeout(() => done(1, "timed out waiting for roster push"), 10_000);

socket.addEventListener("message", async (event) => {
  const message = JSON.parse(event.data);
  seen.push(message.type);

  if (message.type === "welcome") {
    if (message.self !== PARTICIPANT) done(1, `welcome carried the wrong id: ${message.self}`);
    // A second participant joining must reach us as a roster push.
    const response = await fetch(`${BASE}/rooms/${ROOM}/join`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: "smoke-two" }),
    });
    if (!response.ok) done(1, `second join failed: ${response.status}`);
    return;
  }

  if (message.type === "roster") {
    if (message.participants.length !== 2) {
      done(1, `roster had ${message.participants.length} participants, expected 2`);
    }
    clearTimeout(timer);
    socket.close();
    done(0, "");
  }
});

socket.addEventListener("error", () => done(1, "websocket errored"));
' && pass "GET /rooms/:code/ws (welcome + roster push)"

# --- capacity ----------------------------------------------------------------
# The smoke socket above has closed by now, so how many participants are left is
# not worth predicting: keep joining until the room says no, then check it said
# no at exactly eight.
rejected=""
occupancy=0
for i in $(seq 1 12); do
  out="$(join "smoke-cap-$i")"
  code="$(tail -n1 <<<"$out")"
  if [[ "$code" == "409" ]]; then
    rejected="$out"
    break
  fi
  [[ "$code" == "200" ]] || fail "join returned $code: $(sed '$d' <<<"$out")"
  occupancy="$(sed '$d' <<<"$out" | node -e 'let d="";process.stdin.on("data",c=>d+=c).on("end",()=>console.log(JSON.parse(d).participants.length))')"
done

[[ -n "$rejected" ]] || fail "the room never filled up"
[[ "$occupancy" == "8" ]] || fail "the cap tripped at $occupancy participants, expected 8"
grep -q room_full <<<"$rejected" || fail "a full room did not report room_full"
pass "room caps at 8 (the next join gets 409 room_full)"

echo "all checks passed"
