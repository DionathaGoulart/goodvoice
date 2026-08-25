# Self-hosting goodvoice

goodvoice has no servers of its own. The client talks to a Cloudflare Worker
that you deploy, which signs credentials for Cloudflare's Realtime SFU — the
thing that actually carries the audio. Rooms live in a Durable Object's memory
and nowhere else: there is no database to provision, nothing to migrate and
nothing to back up.

About fifteen minutes, most of it waiting for `npm ci`.

**You need:** a Cloudflare account (the free plan is enough at squad scale),
Node 22 or newer, and a terminal. **You do not need:** a paid plan, a domain, a
credit card at signup, or the Rust toolchain — the client you already have can
be pointed at your Worker from its own settings screen.

---

## 1. Create a Realtime app

Cloudflare's SFU is the part that mixes and fans out the audio. It is called
**Realtime** in the dashboard (it was **Calls** until recently, and the API
still lives at `rtc.live.cloudflare.com`).

1. Sign in at [dash.cloudflare.com](https://dash.cloudflare.com).
2. Find **Realtime** in the sidebar, then **SFU**, then **Create**.
3. Name it anything — `goodvoice` is a reasonable choice; the name is never
   seen by anyone in a call.
4. Copy the two things it gives you: an **App ID** and an **App Secret**. The
   secret is shown once.

If the dashboard has been rearranged since this was written, what you are
looking for is the pair of credentials that authorises
`POST https://rtc.live.cloudflare.com/v1/apps/{APP_ID}/sessions/new` with
`Authorization: Bearer {APP_SECRET}`. That request, and nothing else about the
page it came from, is what the Worker makes.

## 2. Deploy the Worker

```bash
git clone https://github.com/DionathaGoulart/goodvoice
cd goodvoice/server
npm ci

# Log the Wrangler CLI into the account you just used. A browser window opens.
npx wrangler login

# The two secrets from step 1. Each command prompts for the value and stores it
# encrypted in Cloudflare — nothing is written to the repo.
npx wrangler secret put CALLS_APP_ID
npx wrangler secret put CALLS_APP_SECRET

npx wrangler deploy
```

The last command prints your Worker's address:

```
Deployed goodvoice triggers (0.62 sec)
  https://goodvoice.<your-subdomain>.workers.dev
```

**Keep that URL.** It is the only thing you and everyone in your squad have to
know.

> Prefer an API token to a browser login — on a CI runner, say? Copy
> `server/.env.example` to `server/.env` and fill in `CLOUDFLARE_API_TOKEN`
> (the "Edit Cloudflare Workers" template) and `CLOUDFLARE_ACCOUNT_ID`.
> Wrangler reads both and skips `wrangler login`.

## 3. Check it before you trust it

```bash
bash server/scripts/smoke.sh https://goodvoice.<your-subdomain>.workers.dev
```

```
goodvoice smoke test against https://goodvoice.<subdomain>.workers.dev (room smoke-1787622941)
  ok   GET /health
  ok   POST /rooms/:code/join (session + ICE servers returned)
  ok   GET /rooms/:code/ws (welcome + roster push)
  ok   room caps at 8 (the next join gets 409 room_full)
all checks passed
```

Line by line: the Worker is up; it can reach Cloudflare's SFU with _your_
credentials and got a session back with ICE servers; the room's WebSocket
pushes the roster when somebody joins; and the eight-person cap holds. The
second line is the one that fails if a secret is wrong — it will say
`sfu_unavailable`.

## 4. Point the client at it

In goodvoice: **settings** → **server** → paste the URL → **use this server**.

That is the whole of it. The client remembers the choice in
`%APPDATA%\art.good.goodvoice\settings.json` and joins there from then on,
including when it is launched by a shortcut or an invite link rather than by a
person. **Back to the bundled one** puts it back.

Everyone who wants to talk to you does the same thing with the same installer.
Two clients pointed at different Workers are two clients in different rooms,
even if they type the same room code.

The box refuses anything that is not an origin, which is nearly always the
dashboard URL pasted by mistake: it wants `https://goodvoice.<subdomain>.workers.dev`,
not `https://dash.cloudflare.com/…/workers/services/view/goodvoice`.

A call already in progress stays on the server it was made on. The next one
goes to the new address.

## 5. Add TURN, when somebody cannot connect

Skip this until you need it. Most calls connect with STUN alone, which the
Worker hands out from Cloudflare's public servers without any credentials at
all.

What it looks like when you need it: everyone can hear everyone _except_ one
person, who joins the room, appears in the roster, and is silent both ways.
That is usually a symmetric NAT — some mobile hotspots, some corporate
networks, some CGNAT — and a relay is the only way through it.

TURN uses **its own credential pair**, not the Realtime app's (this surprised
us too — see DR-1 in `.harness/plan.md`):

1. Dashboard → **Realtime** → **TURN** → **Create**.
2. Copy the **Key ID** and the **API Token**.

```bash
npx wrangler secret put TURN_KEY_ID
npx wrangler secret put TURN_KEY_API_TOKEN
npx wrangler deploy
```

Both are optional and they only work as a pair. If the TURN request fails at
runtime the Worker falls back to STUN rather than failing the join: losing
relay candidates must never drop a call that would have connected without them.

## 6. Knobs worth knowing about

In `server/wrangler.toml`:

|                     | default     | what it does                            |
| ------------------- | ----------- | --------------------------------------- |
| `MAX_AUDIO_BITRATE` | `32000`     | Opus ceiling, bits a second, per person |
| `MAX_VIDEO_BITRATE` | `2500000`   | H.264 ceiling for a screen share        |
| `name`              | `goodvoice` | the first part of the Worker's URL      |

Redeploy after changing any of them. The eight-person room cap is not a knob:
it is in the Durable Object, and prd.md §8 explains why it is a number rather
than a setting.

## What this costs

Nothing, at squad scale, on Cloudflare's free plan — but the free plan is
Cloudflare's to define and the numbers move, so check the current limits on
Workers, Durable Objects and Realtime rather than trusting a figure written
here. What is worth knowing is the shape of the bill: rooms hold no storage and
write nothing, so the Worker's cost is requests and the SFU's is egress —
roughly a squad's talking time, and a great deal more if somebody shares a
screen all evening.

The Durable Object is declared as a SQLite-backed class in `wrangler.toml`
because that is the only kind the free plan offers. goodvoice never touches the
storage API; rooms are ephemeral by design, and a Worker that restarts empties
every room on it.

## When something is wrong

| what you see                                          | what it is                                                                                                                                                                                                             |
| ----------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sfu_unavailable` on join                             | `CALLS_APP_ID` / `CALLS_APP_SECRET` wrong, or the Realtime app was deleted. Re-`secret put` both and redeploy.                                                                                                         |
| `room_full`                                           | eight people are already in that room code.                                                                                                                                                                            |
| The smoke test's first line fails                     | the Worker is not deployed, or the URL has a typo. `npx wrangler deployments list` says what is live.                                                                                                                  |
| `$'\r': command not found` running the smoke test     | the repo was cloned with `core.autocrlf=true` and Git rewrote the script's line endings. `git config core.autocrlf input` and re-clone; the repo's `.gitattributes` prevents it for anything cloned after August 2026. |
| One person is silent both ways, everyone else is fine | step 5.                                                                                                                                                                                                                |
| The client joins, but nobody else is there            | two people pointed at different Workers. Compare the URLs in **settings** → **server**.                                                                                                                                |

---

## What has been verified, and what has not

The client's half is measured: `docs/testing/server-setting.ps1` types a URL
into the settings screen, restarts the app, and checks where it actually joins
— including that a client pointed at a Worker that does not exist **fails
there** rather than quietly falling back to the bundled address. The smoke test
above is a real run against a live deploy, from Git Bash on Windows, which is
what a self-hoster on this project's target platform has.

**Not verified: this document, followed from the beginning by somebody with a
fresh Cloudflare account.** Steps 1 and 5 are dashboard journeys nobody here
has taken with new eyes, and that walkthrough is what plan.md task 6.1 asks
for. If you are that person and something here was wrong, that is a bug in this
file — please say so.
