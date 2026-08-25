# What sharing a screen costs the game on it

plan.md task 5.5. prd.md §4 budgeted **~0 FPS impact while sharing**, on the
grounds that a voice client which costs a game frames is a voice client people
turn off. This is that number, measured, and it was **not ~0**: a 1080p30
hardware share costs a GPU-bound game **5.6% of its frame rate**. The budget is
that measurement now — **≤ 6% at 1080p30** — decided in plan.md §7.1 and
recorded at the end of DR-35, along with the two ways to make it smaller and
why neither is a v0.1.0 change.

## Methodology

`screenshare-bench.ps1` takes three PresentMon captures in one run: the game on
its own, the game with a share live, and the game on its own again.

```powershell
# PresentMon opens an ETW trace session — run this from an elevated shell, or
# it fails on the first capture with "access denied".
powershell -NoProfile -ExecutionPolicy Bypass -File docs\perf\screenshare-bench.ps1 `
  -Process FarFarWest-Win64-Shipping.exe -Seconds 30
powershell ... -Quality 720
```

**The third capture is the point.** A GPU that has warmed up, a scene that has
drifted, a background process that woke up — all of them look exactly like the
share costing frames. The gap between the two idle captures is what this
measurement's own noise is worth, and any claim smaller than it is not a claim.

**Frame times, not frame rates.** The mean of a set of rates is not the rate;
the script averages `MsBetweenPresents` and converts once. The 1% low is the
99th percentile of the same column, which is where a stutter lives.

**`MsGPUBusy` is the honest column.** A game at a frame cap absorbs an added
cost without losing a frame, and one that is GPU-bound loses frames instead —
the same cost, two different-looking answers. GPU milliseconds a frame is the
cost itself, and it is what makes the four runs below comparable.

## The machine, the game, the numbers

DR-12's machine (Windows 11, NVENC — see DR-32), *Far Far West*, a steady scene
at roughly 57 fps with the GPU 95% busy: a genuinely GPU-bound game, which is
the case the budget is about. 30 s a capture, ~1700 frames each.

| share | fps alone | fps sharing | delta | GPU ms alone | GPU ms sharing | idle-to-idle noise |
|---|---|---|---|---|---|---|
| 1080p30 | 57.0 | 53.8 | **−3.2 (−5.6%)** | 16.64 | 17.57 (**+0.93**) | −0.5 fps |
| 720p30 | 56.9 | 54.4 | **−2.5 (−4.4%)** | 16.66 | 17.32 (**+0.66**) | −0.4 fps |
| 1080p15 | 56.5 | 54.7 | −1.8 (−3.2%) | 16.77 | 17.25 (**+0.48**) | −2.0 fps |

The frame-rate deltas in the first two rows are five to eight times the noise
in their own runs. The third row's is not — that run drifted — which is exactly
why the GPU column is there: **0.93 → 0.48 ms when the share rate halves** is
linear, unambiguous, and the same conclusion the frame rate was reaching for.

## Where it goes

Per second of wall clock, a 1080p30 share costs 0.93 ms × 57 frames ≈ **53 ms
of GPU time** for 30 shared frames: **1.8 ms a shared frame**. DR-32 measured
NVENC itself at 0.42 ms a frame. So the encode is under a quarter of it and the
rest is what happens on either side of it — WGC handing over a 1920×1080 BGRA
texture and the video processor turning it into NV12 (DR-31: no hardware
encoder here takes BGRA, so that conversion is not optional).

That is also why **720p saves so little**: both qualities capture and read the
same 1920×1080 source. Only the scale and the encode get cheaper, and those are
the small half.

## What would make it ~0, in the order worth trying

1. **Share at fewer frames a second.** Measured above: halving the rate halves
   the cost, because every part of it is per-frame. 15 fps is visibly choppier
   for a game and perfectly fine for a document; a share that picks its rate by
   what it is looking at would pay 0.48 ms rather than 0.93 for the case that
   matters here.
2. **Stop paying for frames nobody sees.** The capture thread drops frames that
   arrive faster than `SHARE_FPS` *after* WGC has produced them, but the
   conversion and encode only run on kept frames — so this is already done, and
   the remaining per-frame cost is the floor for the rate chosen in (1).
3. **Look at the conversion.** 1.4 ms a frame for a capture and a colour
   convert on a card that encodes in 0.42 ms is worth an explanation before it
   is worth an optimisation. `VideoProcessorBlt` per frame at source resolution
   is the suspect; a shader doing scale-and-convert in one pass, at the
   *output* resolution, is the alternative.
4. **Revisit the budget.** "~0" is a number nobody measured before this. 5.6%
   of a frame rate on the machine's own worst case, for a feature the user
   turned on deliberately and can turn off, may be the right trade — but that
   is a decision to make with the numbers rather than an assumption to keep.

Nothing here is decided: this document is the measurement, and DR-35 is where
the choice will be written down.
