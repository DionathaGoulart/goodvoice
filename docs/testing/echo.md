# Testing the echo canceller in a real room

Task 3.4 put WebRTC's AEC3 between the microphone and the encoder and measured
it against a **synthetic** loopback: the render stream handed straight back to
the capture side, cancelled by 31.8 dB (`processing.rs`,
`what_the_speakers_played_does_not_come_back`). That is the hard case for a
canceller and it is not the real one. In the real one the reference arrives at
the microphone late, quiet, filtered by a transducer and smeared by a room, and
the delay estimator has to find it.

plan.md §7.6 is the row that owes the real one, and prd.md §3 F4 makes it block
the release.

| | scripted | by hand |
|---|---|---|
| A loudspeaker's echo, measured at the far end | ✅ `echo-room` | |
| The canceller's own view of it | ✅ `echo-room` | |
| Whether the leftovers *sound* like an echo | | ⬜ `--record`, then listen |
| Whether the suppressor sounds better on than off (4.7) | | ⬜ `--record`, then listen |

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
  quiet            599      -7.2 dB   56.1 dB below sent        542.9   0.11
  suppressed       600      -3.7 dB   59.4 dB below sent        530.0   0.12
  echo off         600      -5.3 dB   55.4 dB below sent        462.1   0.16
  echo on          600      -3.4 dB   69.3 dB below sent         98.4   0.19
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

## The state of it, 2026-08-25

**Refused, twice, on the machine this repository is developed on.** COUPLING
came out at 0.4 dB and then 2.0 dB against the 6 dB the drill needs, and the
reason is not subtle: **this machine has no loudspeaker.**

```text
## render endpoints

- **Headset Earphone (HyperX Virtual Surround Sound)** — 48000 Hz, 2 ch, 32-bit; …

Present and not active:

- Speakers (High Definition Audio Device) — not present (nothing in the jack)
- Speakers (fifine Microphone) — disabled
- Headphones (High Definition Audio Device) — not present (nothing in the jack)
- 24G2W1G4 (NVIDIA High Definition Audio) — disabled
- … eight more, all `not present`

## capture endpoints

- **Microphone (fifine Microphone)** — 48000 Hz, 2 ch, 32-bit; …
```

That is `cargo run -p goodvoice-harness --bin probe`, which lists the
endpoints a machine *has* and not only the ones it is using — a jack with
nothing in it reports `not present` rather than disappearing, so a machine that
had a loudspeaker last week and has none today looks identical to one that
never had one unless the inactive ones are printed too.

The one thing that makes sound here is a headset earcup, and an earcup that is
not held against the microphone is not a room. DR-23 got its acoustic round
trip out of this same pair of devices *by hand*, with the earcup held against
the microphone, and said so.

Two independent readings agree, which is why this is written down as a fact
about the room rather than as a drill that needs more work. The drill's own
per-window standout says 2.0 dB; a whole-file spectrum of the recordings,
computed outside this repository, puts the room's energy at 200–450 Hz and
finds the 1200 Hz bin **below** its neighbours in the segment where the tone
was playing.

**What clears it.** A loudspeaker in front of the microphone, then one command:

```powershell
cargo run -p goodvoice-harness --bin echo-room -- --record C:\Users\you\heard
```

Either an earcup laid face-down against the fifine, which is DR-23's
arrangement and enough for the measurement, or — better, because it is what a
person on speakers actually has — a speaker in the onboard analog jack, which
brings `Speakers (High Definition Audio Device)` back to `active`. There is a
third: the fifine has a headphone jack of its own, and Windows carries it as
`Speakers (fifine Microphone)` in the `disabled` list. Anything plugged into
*that* is a loudspeaker a centimetre from the capsule, which is the strongest
echo path this hardware can be made to have.

**One thing the room did to the measurement, worth knowing before the re-run.**
It was never quiet: the recordings peak at 3 000–8 500 of 32 767 with the
loudspeaker silent, and their energy sits in a 200–450 Hz rumble. That is the
noise the tone has to stand out of, and 200 ms windows were chosen because of
it. A quieter room makes the same drill much sharper.

## By hand

Two questions here have no instrument, and both are answered by listening to
what `--record` wrote. It writes one WAV per segment — mono, 16-bit, 48 kHz,
exactly what the far end received — and it writes them **even when the run
refuses a verdict**, so the suppressor half below does not wait on a
loudspeaker.

1. **Does the residual sound like an echo?** `echo-on.wav`, against
   `echo-off.wav`. A number under a noise floor says how much is left, not
   whether what is left is a voice repeating itself.
2. **Is the noise suppressor better on than off?** (task 4.7's outstanding
   row.) `quiet.wav` against `suppressed.wav`, back to back, same room fifteen
   seconds apart. The level is not the answer and the drill says so: the gain
   controller runs *after* the suppressor and is not one of the switches, so it
   answers a quieter frame by turning it up. `NOISE_SUPPRESSED` came out at
   −1.3 dB, 0.1 dB and −0.4 dB across three runs of the same room — which is
   the sound of a measurement that is not measuring the thing.

The recordings are the room they were taken in, so they are not committed here.
