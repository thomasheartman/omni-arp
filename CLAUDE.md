# omni-arp

CLAP note-effect arpeggiator for Bitwig, built on nice-plug. README.md is the user-facing
reference (parameters, pattern rules, edge cases); this file is for working on the code.

## Commands

```shell
cargo test
cargo clippy --all-targets
cargo xtask bundle omni-arp --release    # -> target/bundled/omni-arp.clap
rm -rf ~/Library/Audio/Plug-Ins/CLAP/omni-arp.clap && cp -R target/bundled/omni-arp.clap ~/Library/Audio/Plug-Ins/CLAP/
```

To validate, build clap-validator into /tmp (`cargo install --git
https://github.com/free-audio/clap-validator --root /tmp/clapval clap-validator`) and run
`/tmp/clapval/bin/clap-validator validate target/bundled/omni-arp.clap` on a debug bundle, which
aborts if `process()` allocates. Expect 36 passed, 8 skipped (audio ports and presets).

## Code map

- `src/pattern.rs`: the pure generator. A `Spec` (the pattern knobs) plus note and octave counts
  and a seed give one cycle of indices into the octave-stacked pool. Everything is generated
  going up and mirrored for Down. Its only link to nice-plug is the `Enum` derives.
- `src/arp.rs`: the engine, with no host dependencies. Pool and latch, the key range the
  pattern's indices point into (`range`), step clock (tempo grid, free-running, triggers), note
  endings (`Until`), the Notes voicing, and everything applied at
  playback rather than in the pattern: Repeats, Echo steps, Steps (`length`), and a fresh
  Shuffle seed each cycle.
- `src/lib.rs`: parameters, `Settings` from parameters, MIDI routing (trigger channel; every
  note-off calls both `release` and `key_off`), conversion to nice-plug events, and the CLAP
  remote-control pages.

## Rules

- Version control is jj. Finished changes get a `jj commit` with a descriptive message.
- The realtime path never allocates. Buffers are preallocated for the worst case (128 keys, 7
  octaves, `pattern::max_len`). `the_realtime_path_never_allocates` enforces it through
  nice-assert-no-alloc; extend it when adding code paths.
- nice-plug is pinned to `=0.4.2` and nice-plug-xtask to `=0.1.1`. The API differs from
  NIH-plug, so check the nice-plug source rather than recalling NIH-plug.
- CLAP only; VST3 was dropped on purpose.
- `SAMPLE_ACCURATE_AUTOMATION` stays off: nice-plug's CLAP wrapper would add the sub-block offset
  to transport positions that are already current after a transport event.
- No stuck notes. Anything that stops the arp (pool empty, transport start or stop, reset) ends
  every sounding note. `reset()` can't send events, so it empties the pool and the next tick
  ends the notes.
- The pattern property tests run every knob combination (in range, covers every note, no note
  twice in a row). New shapes also get known cycles in `known_cycles`.
- Parameter IDs identify settings in saved Bitwig projects. Don't rename them casually.
- Every parameter sits on a remote-control page (`remote_controls` in `lib.rs`, eight per
  page), and `ArpParams` lists fields in page order. New parameters go in both.
- Parameter names are at most eight characters, distinguishing word first ("Oct Down", not
  "Octaves Down"). CLAP has no separate short name, and Bitwig cuts longer names off on its
  remote controls. The README table gives each one's full meaning.

## Decisions

- Patterns are knobs (Shape, Dir, Start, Edge, Pair, First) rather than a list of modes. Chord
  and As Played were dropped; other devices cover them. Random came back as Shape Shuffle: every
  note once per cycle, a new order each cycle, never starting on the note that just played.
- The two lines are the Lead and the Pair. First defaults to Lead, so Low and High play the walk
  before the pedal note unless First is Pair. With Mirror, First duplicates Dir; that's accepted.
- Repeat x2/x4 were shapes; now Repeats (1-32) applies to every step, Pair steps included.
- Echo Below/Above repeat each lead step (the whole chord with Notes above 1) an octave away.
  A Pair with its own rate was considered and left out: two omni-arps in a Note FX Layer can
  fake it.
- Notes voicing ignores walker and direction: every step's own note goes on top. Flipping it for
  the mirrored walker (note at the bottom) is an option nobody has asked for yet.
- Walks stop after the last whole group (pair, triple) that fits, so they end on the top note
  and Reverse is symmetric. Exception: Stairs over exactly three notes keeps its partial pair
  (`0 2 1`), since whole pairs would skip the middle note.
- Oct Mode Thin drops a note from the stacked octave copies only where it would repeat the note
  before it (C E G C' + an octave: `C E G C' E' G' C''`). No sorting, no other deduplication.
  1 by 1 and Alt keep whole copies; with 1 by 1 you hear which octave you're in.
- Pair Mirror keeps each walker in its half of the range. An earlier rule, stopping the walkers
  before they cross, skipped notes with the zig-zag shapes.
- Reverse drops a turnaround step (a note, or a round of both Mirror walkers) that would play
  twice in a row, unless Ends x2 is on.
- Oct Mode: Thin (default, the same as Bitwig's arp), 1 by 1 (Omnisphere's manual), Alt.
  Bitwig's Broad was left out.
- Gate above 100 % overlaps notes. A key that comes round while still on is ended first,
  since MIDI can't hold one key twice.
- Trigger mode tells triggers apart by MIDI channel because nice-plug gives a plugin one note
  input. Each trigger's note-off ends the notes it started, and with Vel Mode As Played the
  trigger sets the velocity. (A separate From Trigger velocity mode was removed as redundant.)
- Custom step patterns (arbitrary intervals) would need a GUI, so they're postponed. New shapes
  get added in code on request.
- Accent patterns are postponed with them. Example: a 3+3+2 rhythm with an accent on the first
  step of each group. Bitwig's own arp does this with a drawable velocity lane; without a GUI
  the nearest option would be a knob choosing from preset groupings (4, 3, 3+3+2, ...) plus an
  accent amount.

## Open items

- Runs in Bitwig on macOS, trigger mode included, confirmed by the user on 2026-09-28. In
  Bitwig the Note Receiver has to sit inside a Note FX Layer to merge with the track's own notes.
- Debugging in Bitwig: `~/Library/Logs/Bitwig/engine.log` has plugin-host events but not the
  plugin's output. A temporary probe that appends to a file in `/tmp` works; remove it after.
