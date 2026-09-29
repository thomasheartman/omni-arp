# omni-arp

A CLAP note-effect arpeggiator that builds patterns from a few knobs: a shape (straight, stairs,
groups of three, shuffle), a direction, where the walk starts, what it does at the edge, and an
optional second line: a mirrored walker, a pedal note, or an echo. Built on
[nice-plug](https://codeberg.org/RustAudio/nice-plug) 0.4.2 (pinned exactly in `Cargo.toml`). It
has no GUI; every setting is a plain parameter, and Bitwig shows them on three remote-control
pages: Pattern, Notes and More.

## Build

```shell
cargo test                               # pattern, engine and timing tests
cargo xtask bundle omni-arp --release    # -> target/bundled/omni-arp.clap
```

Install by copying the bundle to the user CLAP folder, which Bitwig scans by default:

```shell
mkdir -p ~/Library/Audio/Plug-Ins/CLAP
rm -rf ~/Library/Audio/Plug-Ins/CLAP/omni-arp.clap
cp -R target/bundled/omni-arp.clap ~/Library/Audio/Plug-Ins/CLAP/
```

Then restart Bitwig (or rescan plug-ins in its settings) and put omni-arp before an instrument
in the device chain. A debug bundle (`cargo xtask bundle omni-arp`) aborts if `process()` ever
allocates.

## Parameters

Names are kept to eight characters so Bitwig's remote controls show them whole.

| Name     | What it does                                                            | Default   |
| -------- | ----------------------------------------------------------------------- | --------- |
| **Pattern page** |                                                                 |           |
| Shape    | Straight (+1), Stairs (+2 -1), Groups of Three (+1 +1 -1), Shuffle      | Stairs    |
| Dir      | Direction: Up, Down                                                     | Up        |
| Start    | Outside, Middle                                                         | Outside   |
| Edge     | Restart, Reverse, Wrap                                                  | Restart   |
| Pair     | Second line: Off, Mirror, Low, High, Echo Below, Echo Above             | Off       |
| First    | Which line plays first: Lead, Pair                                      | Lead      |
| Repeats  | 1–32 plays of every step                                                | 1         |
| Rate     | 1/4, 1/8, 1/8T, 1/16, 1/16T, 1/32                                       | 1/16      |
| **Notes page** |                                                                   |           |
| Notes    | 1, 2, 3, All: the pattern's note on top, plus the next held notes below it | 1      |
| Chrd Vel | Chord velocity: 1–100 % of the top note's velocity, for the notes below it | 80 %   |
| Gate     | Note length, 0–200 % of the step                                        | 100 %     |
| Oct Down | Octaves below the held notes, 0–3                                       | 0         |
| Oct Up   | Octaves above the held notes, 0–3                                       | 0         |
| Oct Mode | Octave behavior: Thin, 1 by 1, Alt                                      | Thin      |
| Vel Mode | As Played (in Trigger mode, the trigger's velocity), Fixed              | As Played |
| Velocity | 1–127, used when Vel Mode is Fixed                                      | 100       |
| **More page** |                                                                    |           |
| Steps    | Full, or 1–32 steps before the pattern starts over                      | Full      |
| Ends x2  | Reverse turnarounds play their step twice                               | Off       |
| Advance  | Tempo, Trigger                                                          | Tempo     |
| Trig Ch  | Trigger channel, 1–16: whose notes step the pattern in Trigger mode     | 16        |
| Latch    | Keep playing after the keys are released; the next chord replaces the old one | Off |
| Retrig   | Every chord change restarts the pattern instead of continuing           | Off       |

## How patterns work

The pool is the held notes sorted by pitch, one entry per MIDI key. The octave settings stack
copies of it below and above, and a pattern walks that range. Numbering the notes from 1 (the
lowest), with 6 notes unless noted:

```
Straight Up                           1 2 3 4 5 6
Straight Up, Edge Reverse             1 2 3 4 5 6 5 4 3 2
Straight Up, Start Middle             4 5 6                    Restart plays one half
Straight Up, Start Middle, Edge Wrap  4 5 6 1 2 3
Stairs Up (5 notes)                   1 3 2 4 3 5 4
Groups of Three (5 notes)             1 2 3 2 3 4 3 4 5 4 5
Shuffle (5 notes)                     3 1 5 2 4 | 2 5 1 4 3 | ...   a new order every cycle
Pair Mirror                           1 6 2 5 3 4              Join
Pair Mirror, Down, Start Middle       3 4 2 5 1 6              Spread
Pair Mirror, Edge Reverse             1 6 2 5 3 4 2 5          Join/Spread
Pair Low (5 notes)                    2 1 3 1 4 1 5 1
Pair Low, First Pair (5 notes)        1 2 1 3 1 4 1 5
Pair High, Down (5 notes)             4 5 3 5 2 5 1 5
Pair Echo Below (3 notes)             1 1v 2 2v 3 3v           v: an octave below
```

- **Shape** is how the walk moves from each position: Stairs goes two up and one down, Groups
  of Three plays three in a row and moves up one. Shuffle plays every note once per cycle in a
  new random order each cycle.
- **Dir** Down mirrors the walk top to bottom.
- **Start** Outside begins at the end opposite the direction. Middle begins at the middle note
  on the side the walk is heading: the upper middle going up, the lower middle going down.
- **Edge** decides what happens when the next note would leave the range. Restart starts over
  from the start, Reverse turns around and walks back from the end it reached, and Wrap carries
  on from the other end.
- **Pair** adds a second line that alternates with the lead walk. Mirror is a second walker doing
  the same shape the opposite way; each keeps to its half of the range, so from the outside they
  walk in (Join) and from the middle they walk out (Spread). Low and High play the lowest or
  highest note between every step of a walk over the others. Echo Below and Echo Above play each
  lead step again, an octave away.
- **First** picks which line plays first. With Mirror, it gives the same patterns as flipping Dir.
- **Repeats** plays every step that many times, the pair's steps included.

The generator (`src/pattern.rs`) is a pure function of the knobs and the note count, tested for
every combination of knobs with 1–6 notes and 1–3 octaves.

### Recipes

| Omnisphere / Bitwig    | Shape    | Dir  | Start   | Edge    | Pair   | Also        |
| ---------------------- | -------- | ---- | ------- | ------- | ------ | ----------- |
| Up                     | Straight | Up   | Outside | Restart | Off    |             |
| Down/Up                | Straight | Down | Outside | Reverse | Off    |             |
| Up/Down+               | Straight | Up   | Outside | Reverse | Off    | Ends x2 on  |
| Stairs Up/Down         | Stairs   | Up   | Outside | Reverse | Off    |             |
| Repeat X2              | Straight | Up   | Outside | Restart | Off    | Repeats 2   |
| Random                 | Shuffle  |      |         |         | Off    |             |
| Join (Blossom In)      | Straight | Up   | Outside | Restart | Mirror |             |
| Spread (Blossom Out)   | Straight | Down | Middle  | Restart | Mirror |             |
| Join/Spread            | Straight | Up   | Outside | Reverse | Mirror |             |
| Spread/Join            | Straight | Down | Middle  | Reverse | Mirror |             |
| Low & Down             | Straight | Down | Outside | Restart | Low    | First Pair  |
| Hi & Up/Down           | Straight | Up   | Outside | Reverse | High   | First Pair  |

Chord and As Played aren't here; other devices cover them.

### Oct Mode

With C E G held, Oct Up 1, and Straight Up with Edge Reverse:

```
Thin      C E G C' E' G' E' C' G E    one walk over every octave (what Bitwig's arp does)
1 by 1    C E G E C' E' G' E'         the whole pattern, then again an octave up
Alt       C C' E E' G G' E E'         each step in every octave before the next step
```

Going down, 1 by 1 and Alt take the octaves from the top.

### Notes

Each step can play more than the pattern's note. The pattern's note goes on top. Every other held
note moves into its octave and then drops by octaves until it's below it, and the next highest
of those fill in. With C E G held and Notes set to All, the steps play `C` over `E G` below it,
then `E` over `G C`, then `G` over `C E`: the three inversions. Chrd Vel accents the top note by
playing the others quieter, at 80 % of the top note's velocity by default, however hard their
own keys were pressed. The inversion doesn't depend on the direction or the walker: every step
puts its own note on top. An echo repeats the whole chord an octave away.

## Edge-case decisions

- **Overshoot ends the pass.** A walk stops before the first note that would leave the range,
  then follows the Edge setting. It never clamps or skips. The top note is still reached.
- **Fewer than 3 notes.** Stairs and Groups of Three need three notes to take their steps, and
  walk straight over fewer. A single note just repeats, and so does a Middle + Restart half of
  two.
- **Turnarounds.** When Reverse would play a step twice in a row where the walk turns, or where
  the cycle loops, the step plays once. Ends x2 keeps both. A step is a note, or with Pair
  Mirror a round of both walkers, so Join/Spread plays the middle and outer pairs once per
  cycle. Stairs and Groups of Three never turn on the same note.
- **Shuffle** ignores Start and Edge. A new cycle never starts on the note that ended the last
  one. With Mirror, the lower half (with the middle note, if any) and the upper half are
  shuffled separately and alternate; with Low or High, the pedal note stays put. Each plugin
  instance shuffles with its own seed.
- **Mirror.** The two halves share the middle note when the count is odd, and a note that would
  play twice in a row plays once. The walkers can't pass each other, so Wrap acts as Restart.
- **Low and High.** The pedal note is left out of the walk.
- **Echo.** With First Pair, the echo comes before its note. Echoes past either end of the MIDI
  range are rests. The lead moves on every other step, like with the other pairs.
- **Repeats and Steps.** Repeats applies to every step, so an echoed step with Repeats 2 plays
  `1 1 1v 1v`. Steps counts steps played, repeats included. A pattern shorter than Steps keeps
  cycling until Steps have played: 3 notes with Steps 4 play `1 2 3 1 | 1 2 3 1`.
- **Chord changes** update the pool immediately. By default the pattern continues from the same
  step, clamped to the new pattern's last step if it got shorter. Retrig starts over instead.
  Without latch, a chord played after all keys were up starts at step one.
- **Beyond the MIDI range.** Notes octaves above key 127 or below key 0 are rests, or drop out
  of the chord with Notes above 1. The step is kept so the rhythm doesn't shift.
- **Channels.** The pool keeps one entry per key, and output notes use the channel of the key
  they came from. A note-off only releases a key held on the same channel.

## Timing

- **Transport playing:** a step starts on the first sample whose song position (in quarter
  notes) reaches a multiple of the rate. Note-ons are sample-accurate within the block and
  independent of block size. A chord pressed between steps waits for the next step on the grid.
  Pressing play restarts the pattern and plays the first step right away.
- **Transport stopped:** the arp free-runs from the host tempo (120 BPM if there is none). The
  clock starts on the key press, so the first note sounds immediately.
- **Gate** is a fraction of the step, at least one sample. At 100 % a note ends on the sample
  the next one starts. Above 100 % notes overlap. A key that comes round again while it's still
  on is ended first, because MIDI can't hold one key twice and the old note-off would cut the
  new note short.
- **Note-offs:** releasing every key (without latch), stopping or starting the transport, and
  turning latch off with no keys down all end every sounding note immediately. Stopping then
  waits one step before free-running, so notes the host releases right after the stop don't
  blip.
- **Reset and deactivate:** plugins can only send events from `process()`, so `reset()` empties
  the pool and the note-offs go out on the first sample of the next `process()` call. The host
  always calls `reset()` after reactivating.
- MIDI CCs, pitch bend, channel pressure and program changes pass through unchanged. Polyphonic
  expressions are dropped, since they're tied to the input keys.

## Trigger mode

With Advance set to Trigger, the tempo grid stops driving the arp. Every note-on on the trigger
channel (Trig Ch) plays the next step at that sample, and the notes it starts end with that
trigger's note-off, so Rate and Gate don't apply. Notes on other channels still make up the
chord. With Vel Mode As Played, each step takes its velocity from the trigger note; Fixed still
overrides it.

nice-plug gives a plugin a single note input, which is why triggers are told apart by channel.
To drive the arp from a drum track in Bitwig, put a Note Receiver pulling in the drum track's
notes inside a Note FX Layer ahead of omni-arp, with those notes moved to the trigger channel.
The layer matters: a Note Receiver straight in the chain replaces the track's own notes instead
of merging with them.

## Platform notes (macOS, Apple Silicon)

- Built and tested on macOS arm64. The CLAP bundle passes
  [clap-validator](https://github.com/free-audio/clap-validator) 0.4.1 (36 passed, 0 failed; the
  8 skipped tests cover audio ports and presets), and runs in Bitwig on macOS.
- The bundler signs ad hoc. That's fine for local use; sharing the plugin needs Developer ID
  signing and notarization. For Intel Macs, `cargo xtask bundle-universal omni-arp --release`
  should build a universal binary (needs `rustup target add x86_64-apple-darwin`; untested).
- `SAMPLE_ACCURATE_AUTOMATION` stays off. With it on, nice-plug 0.4.2's CLAP wrapper adds the
  sub-block offset to the song position even when the split came from a transport event whose
  position is already current. That would shift the grid.
