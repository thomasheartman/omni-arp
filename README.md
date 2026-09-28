# Stairs Arp

A CLAP note-effect arpeggiator with Omnisphere 3's note patterns, Stairs included, built on
[nice-plug](https://codeberg.org/RustAudio/nice-plug) 0.4.2 (pinned exactly in `Cargo.toml`).
It has no GUI; every setting is a plain parameter, so Bitwig shows them as device parameters.

## Build

```shell
cargo test                               # pattern, engine and timing tests
cargo xtask bundle stairs_arp --release  # -> target/bundled/Stairs Arp.clap
```

Copy `target/bundled/Stairs Arp.clap` to `~/Library/Audio/Plug-Ins/CLAP/`, or add
`target/bundled` to Bitwig's plugin locations. Put it before an instrument in the device chain.
A debug bundle (`cargo xtask bundle stairs_arp`) aborts if `process()` ever allocates.

## Parameters

| Parameter        | Values                                   | Default   |
| ---------------- | ---------------------------------------- | --------- |
| Mode             | Chord, Up, Down, Up/Down, Down/Up, Random, As Played, Repeat X2, Repeat X4, Join, Spread, Join/Spread, Spread/Join, Stairs Up, Stairs Down, Stairs Up/Down, Stairs Down/Up | Stairs Up |
| Rate             | 1/4, 1/8, 1/8T, 1/16, 1/16T, 1/32        | 1/16      |
| Octaves          | 1–4                                      | 1         |
| Gate             | 1–100 % of the step                      | 50 %      |
| Velocity Mode    | As Played, Fixed                         | As Played |
| Fixed Velocity   | 1–127, used when Velocity Mode is Fixed  | 100       |
| Latch            | Keep playing after the keys are released; the next chord replaces the old one | Off |
| Repeat Ends      | Two-part modes play the turnaround note twice. On, Up/Down and Down/Up are Omnisphere's Up/Down+ and Down/Up+ | Off |
| Restart On Chord | Every chord change restarts the pattern instead of continuing | Off |

## How patterns work

The pool is the held notes sorted by pitch (in press order for As Played), one entry per MIDI
key. With `n` notes and `R` octaves, patterns walk the indices `0..M` with `M = n * R`, and
index `i` plays `pool[i % n] + 12 * (i / n)`. Examples with `M = 5` and `M = 6`:

```
Stairs Up       0 2 1 3 2 4 3                  a[2j] = j, a[2j+1] = j + 2
Stairs Down     4 2 3 1 2 0 1                  Stairs Up mirrored (i -> M - 1 - i)
Stairs Up/Down  0 2 1 3 2 4 3 4 2 3 1 2 0 1    one, then the other
Repeat X2       0 0 1 1 2 2 3 3 4 4
Join            0 5 1 4 2 3                    outside in (the manual's 1-6-2-5-3-4)
Spread          2 3 1 4 0 5                    inside out (the manual's 3-4-2-5-1-6)
```

The other modes:

- **Chord** plays every held note at once on each step, walking up the octaves: the chord, then
  the chord an octave up, and so on.
- **Random** plays each note once per cycle in a new random order every cycle, and never starts
  a cycle on the note that ended the last one. Each plugin instance gets its own seed.
- **As Played** is Up over the notes in the order their keys went down.

The generator (`src/pattern.rs`) is a pure function over `M`, tested for `n` 1–6 and
octaves 1–3.

## Edge-case decisions

- **Overshoot ends the cycle.** A stairs walk stops before the first index that would be `>= M`
  and loops. It never clamps or skips. The top note is still reached (as the last +2), and the
  Up/Down seams never repeat a note: Stairs Up ends on `M - 2`, Stairs Down starts on `M - 1`.
- **Fewer than 3 indices.** A stair can't take its +2 step, so Stairs modes play their plain
  counterparts. With one note (`n = 1, R = 1`) every mode just repeats it.
- **Turnarounds.** In the two-part modes (Up/Down, Join/Spread, Stairs Up/Down and their
  reverses), a note that would play twice in a row where the halves meet or where the cycle
  loops plays once. Repeat Ends keeps both. Only Up/Down, Down/Up, and Join/Spread and
  Spread/Join with an odd `M` have such a note. The Stairs halves never meet on the same note.
- **Odd Join and Spread.** Join ends on the middle note and Spread starts on it, so Spread's
  pairs run low then high: `M = 5` gives Join `0 4 1 3 2` and Spread `2 1 3 0 4`.
- **Chord changes** update the pool immediately. By default the pattern continues from the same
  step, clamped to the new pattern's last step if the pattern got shorter. Restart On Chord
  switches to starting over. Without latch, a chord played after all keys were up starts at
  step one.
- **Octaves above MIDI key 127** are rests, or drop out of the chord in Chord mode. The step is
  kept so the rhythm doesn't shift.
- **Keys on several channels.** The pool keeps one entry per key. Output notes use the channel
  of the key they came from.

## Timing

- **Transport playing:** a step starts on the first sample whose song position (in quarter
  notes) reaches a multiple of the rate. Note-ons are sample-accurate within the block and
  independent of block size. A chord pressed between steps waits for the next step on the grid.
  Pressing play restarts the pattern and plays the first step right away.
- **Transport stopped:** the arp free-runs from the host tempo (120 BPM if there is none). The
  clock starts on the key press, so the first note sounds immediately.
- **Gate** is a fraction of the step length, at least one sample. Every note of a step (one,
  or the whole chord in Chord mode) gets its note-off before the next step's note-ons, on the
  same sample at 100 %.
- **Note-offs:** releasing every key (without latch), stopping or starting the transport, and
  turning latch off with no keys down all end the sounding note immediately. Stopping then
  waits one step before free-running, so notes the host releases right after the stop don't
  blip.
- **Reset and deactivate:** plugins can only send events from `process()`, so `reset()` empties
  the pool and the note-offs go out on the first sample of the next `process()` call. The host
  always calls `reset()` after reactivating.
- MIDI CCs, pitch bend, channel pressure and program changes pass through unchanged. Polyphonic
  expressions are dropped, since they're tied to the input keys.

## Differences from Omnisphere

- **Octaves.** Omnisphere's manual says 2 OCT plays the pattern over the held notes and then
  the same pattern an octave higher. This plugin walks one pattern across all `M` notes
  instead, as specified. The two agree for Chord, Up, As Played and Repeat X2/X4, and differ
  for the rest. Stairs Up over C E G with 2 octaves plays `C G E C' G E' C' G' E'` here, but
  going by the manual `C G E C' G' E'` in Omnisphere.
- **Not included:** the 2/3/4 OCT Alt octave modes, which play each note through every octave
  before moving on, and the Loop, Chaos and Once play modes, which act on Omnisphere's step
  sequencer. This plugin has no step sequencer.

## Platform notes (macOS, Apple Silicon)

- Built and tested on macOS arm64. The CLAP bundle passes
  [clap-validator](https://github.com/free-audio/clap-validator) 0.4.1 (36 passed, 0 failed; the
  8 skipped tests cover audio ports and presets). It hasn't been tried in Bitwig yet.
- The bundler signs ad hoc. That's fine for local use; sharing the plugin needs Developer ID
  signing and notarization. For Intel Macs, `cargo xtask bundle-universal stairs_arp --release`
  should build a universal binary (needs `rustup target add x86_64-apple-darwin`; untested).
- `SAMPLE_ACCURATE_AUTOMATION` stays off. With it on, nice-plug 0.4.2's CLAP wrapper adds the
  sub-block offset to the song position even when the split came from a transport event whose
  position is already current. That would shift the grid.
