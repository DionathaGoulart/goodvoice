# Testing auto-reconnect

goodvoice rebuilds a call that drops rather than ending it (plan.md task 3.5).
There are two ways to check that, and they check different things.

| | `reconnect-drill` | netdown |
|---|---|---|
| What dies | the session, on request | the network, for real |
| Runs on | any host, any time | one host, needs `sudo` |
| Proves | rejoin, republish, resubscribe | the above **plus** that the client notices |
| Takes | ~30 s | ~2 min of babysitting |

Run the drill on every change to `rtc/`. Run netdown when the reconnect
*trigger* changes — the ICE grace, the publish failure limit — because that is
the half the drill cannot see.

## The drill

```sh
cd client/src-tauri
cargo run -p goodvoice-harness --release --bin reconnect-drill
# or against a Worker of your own:
cargo run -p goodvoice-harness --release --bin reconnect-drill -- --base http://localhost:8787
```

Two clients join a fresh room and hold a conversation. One of them throws its
seat away ([`Call::drop_session`]), and the drill fails unless all three of
these happen on their own:

1. the client that dropped ends up **live again with a new participant id** —
   a reconnect takes a new seat, so an unchanged id means nothing happened;
2. it is audible again — the microphone was republished onto the new session;
3. the roommate who did nothing hears it again — their subscription followed
   the new session id instead of staying pointed at the dead one (DR-8).

Expect `not_found_track_error` lines in the output. A publisher is announced on
the roster before Cloudflare will serve them, and the retry is the fix, not a
symptom (DR-8). `track refers to a session outside this room` is the same class
of race seen from the other side, and the two-second reconcile clears it.

## Netdown, for real

macOS, with `pfctl`. This drops **all** traffic to Cloudflare's Realtime edge
and the Worker for ten seconds, which is what a lost link looks like from the
client's side. It affects the whole machine, so close anything that would mind.

```sh
# 1. start a call in one terminal and check you can hear the other end
cargo run -p goodvoice-harness --release --bin call -- --room netdown-test

# 2. in another terminal, take the network away for ten seconds
cat <<'RULES' | sudo pfctl -f - -E
block drop out proto udp from any to any port 1000:65535
block drop out proto tcp from any to any port 443
RULES
sleep 10
sudo pfctl -d
```

What should happen, in order:

1. the microphone's frames stop landing — within a second the client logs
   `call dropped (…); reconnecting`;
2. the UI shows `reconnecting… (attempt 1)`, then 2 if the first is too early;
3. once `pfctl -d` restores the link, the client rejoins **the same room code**
   and audio resumes without anyone touching the app.

If the link stays down past the retry schedule — about 90 seconds
(`reconnect.rs`) — the call ends with `unreachable` and says so. That is the
intended ending, not a failure: a client left running on a dead link should
stop rather than spin overnight.

### Windows

The same test with `netsh`, on the host the client actually ships for:

```powershell
netsh interface set interface "Ethernet" admin=disable
timeout /t 10
netsh interface set interface "Ethernet" admin=enable
```

Not yet run — see plan.md task 3.5's note. Nothing on this path is
Windows-specific (the transport is pure Rust, DR-7), but "the client notices a
dead link" depends on the host's socket behaviour, and that is worth seeing on
the target before the box is ticked.

## A redeploy ends every call in progress

Not a bug, and worth knowing before it looks like one. Room state is in memory
only, so `wrangler deploy` — or a `wrangler secret put`, which publishes a
version of its own — restarts every Durable Object and wipes every roster
(DR-5). Clients see it as a drop and rejoin into an empty room. If you are
testing reconnect and rooms keep coming back empty, check whether something is
deploying underneath you.

[`Call::drop_session`]: ../../client/src-tauri/src/rtc/session.rs
