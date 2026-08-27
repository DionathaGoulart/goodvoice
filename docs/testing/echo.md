# Testing the echo canceller in a real room

Task 3.4 put WebRTC's AEC3 between the microphone and the encoder and measured
it against a **synthetic** loopback: the render stream handed straight back to
the capture side, cancelled by 31.8 dB (`processing.rs`,
`what_the_speakers_played_does_not_come_back`). That is the hard case for a
canceller and it is not the real one. In the real one the reference arrives at
the microphone late, quiet, filtered by a transducer and smeared by a room, and
the delay estimator has to find it.

plan.md §7.6 is the row that owed the real one, and prd.md §3 F4 made it block
the release. It was paid on 2026-08-26 — *the state of it* below — after being
refused twice the day before for want of a loudspeaker.

| | scripted | by hand |
|---|---|---|
| A loudspeaker's echo, measured at the far end | ✅ `echo-room` | |
| The canceller's own view of it | ✅ `echo-room` | |
| Whether the leftovers *sound* like an echo | | ✅ listened 2026-08-26 |
| Whether the suppressor sounds better on than off (4.7) | | ✅ listened 2026-08-26 |

## The drill

```powershell
. $env:USERPROFILE\gv\env.ps1
cargo run -p goodvoice-harness --bin echo-room
cargo run -p goodvoice-harness --bin echo-room -- --seconds 20 --record C:\Users\you\heard
```

Two clients in one process, the way `bin/latency` does it:

- **the room** holds the real microphone and the real speakers, opened through
  the same `hardware::open` the app uses;
- **the far end** publishes a steady 1200 Hz tone and keeps every frame that
  comes back.

The tone makes the whole trip — through the SFU, out of a physical transducer,
across the air, into a physical microphone, and back through the SFU. Nothing
in the path is stubbed.

It walks four segments, twelve seconds each after three of settling:

| segment | tone | canceller | suppressor | what it measures |
|---|---|---|---|---|
| `quiet` | off | off | off | the room's own energy where the tone will be |
| `suppressed` | off | off | **on** | what the suppressor takes out of that |
| `echo off` | **on** | off | off | **the coupling** |
| `echo on` | **on** | **on** | off | the cancellation |

## Reading it

```text
  segment       frames   stands out   at the tone         room level   canceller
  quiet            600       0.3 dB   72.0 dB below sent         73.7   0.07
  suppressed       600      -2.0 dB   65.9 dB below sent        107.0   0.19
  echo off         599      32.4 dB    5.0 dB below sent       3282.0   0.59
  echo on          600       0.7 dB   40.5 dB below sent        255.1   0.02
```

**`stands out` is the number the verdict is made of**, and everything else in
the table is context. It is the tone's own frequency bin against two bins on
either side of it — 960 Hz and 1500 Hz — read out of the *same* 200 ms of
audio. Those bins hold whatever the room is doing across the band and none of
the tone, so their ratio is what the tone is worth over the room at that
instant, and nothing that happens between one segment and the next can move it.

`at the tone` is the same bin against the tone **as sent**, which is the column
`bin/listener --tone` prints and the one §7.6's definition of done asks for. It
is kept because it is comparable across machines, and it is not what the
verdict reads: see *the first metric was wrong* below.

The summary lines:

```text
COUPLING=2.0 dB          how much the loudspeaker lifted the tone out of this room
ECHO_CANCELLED=…         what the canceller took off that, `echo off` → `echo on`
RESIDUAL=…               where the cancelled tone ended up, against the silent room
CANCELLER_BELIEVED=…     AEC3's own residual-echo likelihood, 0–1
VERDICT=measured | inconclusive
```

`ECHO_CANCELLED` is printed as **at least** whenever the cancelled tone lands
within 1.5 dB of the silent room's own standout. An echo pushed under the
room's noise cannot be measured any further down, and the subtraction would
report a number that is really the noise floor's.

## The first metric was wrong, and it said yes

The obvious measurement is the tone's bin in `echo off` against the tone's bin
in `quiet`: how much did the loudspeaker lift that frequency. That is what this
drill did first, and on its first run it reported

```text
COUPLING=17.2 dB over the silent room
```

There was no coupling. The room simply got louder between the two segments —
its median level went 121.0 → 453.3 — and a bin that rises with everything else
rose with it. Fifteen seconds apart is long enough for a room to change, and
the gain controller moves the rest.

Reading the tone against **its own neighbours in the same 200 ms** cannot be
fooled that way: a chair, a fan or a gain change lifts all three bins together
and the ratio does not move. The same run, measured that way, gave 0.4 dB.

Both columns are still printed, because the absolute one is what
`bin/listener --tone` reports and the one comparable between machines. Only the
self-normalising one decides anything.

## The control, which is the whole point

**A canceller that works and a room with no loudspeaker in it produce the same
number.** So the walk turns the canceller *off* first and refuses to report a
cancellation unless the tone stood at least 6 dB out of the room without it:

```text
COUPLING=2.0 dB (the tone stood -5.3 dB out of the room, against -7.2 dB with it silent)
VERDICT=inconclusive
Error: the microphone cannot hear the loudspeaker: the tone stood only 2.0 dB out of
the room, under the 6 dB this needs.
  That is a fact about the room and not about the canceller — nothing here says
  whether it works.
```

This is the same trap §7.5 fell into and caught: a capture device that will not
open gives exactly the column of zeroes a working feature gives. A drill that
cannot tell those apart is worse than no drill, because it reports a pass.

## The state of it, 2026-08-26

**Measured, twice, and the canceller works through a real acoustic path.** The
loudspeaker is DR-23's arrangement — a HyperX earcup laid face-down against the
fifine's capsule, resting on it rather than held — and with one in the room the
drill reaches a verdict instead of refusing one.

```text
                              run A         run B
COUPLING                      32.1 dB       34.7 dB
  standout, canceller off     32.4 dB       32.2 dB
  standout, canceller on       0.7 dB        0.5 dB
  standout, room silent        0.3 dB       -2.5 dB
ECHO_CANCELLED              ≥ 31.7 dB       31.7 dB
at the tone, canceller off     5.0 dB        5.0 dB   below sent
at the tone, canceller on     40.5 dB       41.2 dB   below sent
CANCELLER_BELIEVED        0.59 → 0.02   0.59 → 0.02
VERDICT                      measured      measured
```

**The two witnesses agree, and they are independent.** The far end says the
tone stood 32 dB out of the room with the canceller off and 0.6 dB with it on;
the near side's own `Microphone::echo_likelihood` — which is AEC3's opinion and
never leaves the capturing process — went 0.59 to 0.02 across the same switch,
twice. One of those numbers is a spectrum computed by this drill from received
audio and the other is WebRTC's internal estimate; nothing connects them but
the room.

**31.7 dB through a room, against 31.8 dB against the synthetic loopback.** The
number the real path gives back is the number task 3.4 measured with the
reference handed straight back with no delay, to a tenth of a decibel — which
is the answer to the question DR-41 left open about whether AEC3's delay
estimator finds a *real* reference at all. It does, on this path. **On this
path** is the caveat: the transducer was against the capsule, so the delay to
find was the device pipeline's — DR-23 measured this pair's acoustic round trip
at 84.7 ms — and not a metre of air on top of it. A far-field loudspeaker is a
longer delay and a smeared reference, and nobody has run one.

**`ECHO_CANCELLED` is floored, not saturated.** Run A prints *at least*, because
0.7 dB of residual standout lands within 1.5 dB of the 0.3 dB the silent room
stood at: the echo is under the room's own noise and the subtraction below that
would be reporting the noise floor. Run B, whose silent room sat at -2.5 dB,
had room to print the number outright and printed the same one.

**Conditions, because DR-41 has no record of them.** The render endpoint's
master volume was **100% and unmuted** for both runs, read out of
`IAudioEndpointVolume` before the first one. The refusals of 2026-08-25 were
never checked that way, so nothing here says the earcup was at full volume
then; what it does say is that gain is not the explanation for the pass.

**Two blemishes in the logs, neither of which moves a number.** Run A printed
one `audio device error: A buffer underrun or overrun occurred.` during the
`suppressed` segment and lost a single frame from `echo off` (599 of 600).
A frame in eight hundred is below anything the standout is read at.

### Before that: refused twice, 2026-08-25

Kept because the refusal is what makes the pass worth anything. COUPLING came
out at 0.4 dB and then 2.0 dB against the 6 dB the drill needs, and the reason
was not the canceller: **the machine had no loudspeaker in it.**

```text
## render endpoints

- **Headset Earphone (HyperX Virtual Surround Sound)** — 48000 Hz, 2 ch, 32-bit; …

Present and not active:

- Speakers (High Definition Audio Device) — not present (nothing in the jack)
- Speakers (fifine Microphone) — disabled
- Headphones (High Definition Audio Device) — not present (nothing in the jack)
- 24G2W1G4 (NVIDIA High Definition Audio) — disabled
- … eight more, all `not present`
```

That is `cargo run -p goodvoice-harness --bin probe`, which lists the
endpoints a machine *has* and not only the ones it is using — a jack with
nothing in it reports `not present` rather than disappearing, so a machine that
had a loudspeaker last week and has none today looks identical to one that
never had one unless the inactive ones are printed too. The one thing that
makes sound here is a headset earcup, and an earcup that nobody has put against
the microphone is not a room. Two independent readings agreed on the refusal
the same way two agree on the pass: the drill's per-window standout said 2.0 dB,
and a whole-file spectrum computed outside this repository found the 1200 Hz bin
*below* its neighbours in the very segment where the tone was playing.

**The endpoint list understates what this machine has.** `probe` prints
`disabled` for three endpoints, and the registry says more than the word does:
`HKLM\…\MMDevices\Audio\Render\{guid}\DeviceState` carries **`0x10000001`**
for them — `DISABLED` with the `ACTIVE` bit still set, which is Windows saying
*plugged in and would work if you turned it on* — against plain `4`
(`NOTPRESENT`) for a jack with nothing in it. Two of the three are candidate
loudspeakers that cost nothing but a click in `mmsys.cpl`: the monitor over
DisplayPort (`24G2W1G4`, NVIDIA HD Audio) and the fifine's own headphone jack
(`Speakers (fifine Microphone)`). The monitor is the one to enable for the
far-field run this doc still owes, since `hardware::open` takes the default
render device and a monitor speaker is a metre of air away.

**One thing the room does to the measurement.** It is never quiet: the
recordings peak at 3 000–8 500 of 32 767 with the loudspeaker silent, and their
energy sits in a 200–450 Hz rumble. That is the noise the tone has to stand out
of, and 200 ms windows were chosen because of it. A quieter room makes the same
drill sharper — which is visible in the two runs above, where the silent room's
own standout moved 2.8 dB between them and took the floor on `ECHO_CANCELLED`
with it.

## By hand

Two questions here have no instrument, and both are answered by listening to
what `--record` wrote. It writes one WAV per segment — mono, 16-bit, 48 kHz,
exactly what the far end received — and it writes them **even when the run
refuses a verdict**, so the suppressor half never waited on a loudspeaker.

Both were listened to on **2026-08-26**, on the recordings of the two runs
above.

1. **Does the residual sound like an echo?** `echo-on.wav` against
   `echo-off.wav`. **A remnant is audible, far under the tone it came from.**
   `echo-off.wav` is a blatant 1 200 Hz tone; what survives the canceller is a
   weak one you can hear if you know to listen for it, not a room repeating
   itself. So the numbers and the ear disagree about *nothing is left* and
   agree about the size: 0.5–0.7 dB of standout is at the room's own noise, and
   at the room's own noise this tone is still findable by a person. It does not
   pump, gate or come and go — the failure modes worth a DR — it is just quiet.
2. **Is the noise suppressor better on than off?** (task 4.7's outstanding
   row — this closes it.) `quiet.wav` against `suppressed.wav`, back to back,
   both runs. **Better on.** The 200–450 Hz rumble the room sits in is clearly
   reduced and nothing that matters goes with it.
   **The level could not have told you that**, and that is the point of asking
   a person: `NOISE_SUPPRESSED` came out at −1.3, 0.1 and −0.4 dB on
   2026-08-25 and then −1.6 and **+4.2** dB on 2026-08-26 — five readings of
   one room, spanning six decibels and both signs. The gain controller runs
   *after* the suppressor and is not one of the switches, so it answers a
   quieter frame by turning it up, and the median level it reports is the sound
   of a measurement that is not measuring the thing.

The recordings are the room they were taken in, so they are not committed here.
