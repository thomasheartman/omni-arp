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
  give one cycle of indices into the octave-stacked pool. Everything is generated going up and
  mirrored for Down. Its only link to nice-plug is the `Enum` derives.
- `src/arp.rs`: the engine, with no host dependencies. Pool and latch, step clock (tempo grid,
  free-running, triggers), note endings (`Until`), the Notes voicing, Length, and the Repeat
  shapes (applied at playback, not in the pattern).
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

## Decisions

- Patterns are knobs (Shape, Direction, Start, Edge, Pair) rather than a list of modes. Chord,
  Random and As Played were dropped; other devices cover them.
- Pair Mirror keeps each walker in its half of the range. An earlier rule, stopping the walkers
  before they cross, skipped notes with the zig-zag shapes.
- Reverse drops a turnaround step (a note, or a round of both Mirror walkers) that would play
  twice in a row, unless Repeat Ends is on.
- Octave Behavior: Thin (default, the same as Bitwig's arp), 1 by 1 (Omnisphere's manual), Alt.
  Bitwig's Broad was left out.
- Note Length above 100 % overlaps notes. A key that comes round while still on is ended first,
  since MIDI can't hold one key twice.
- Trigger mode tells triggers apart by MIDI channel because nice-plug gives a plugin one note
  input. Each trigger's note-off ends the notes it started.
- Custom step patterns (arbitrary intervals) would need a GUI, so they're postponed. New shapes
  get added in code on request.

## Open items

- Runs in Bitwig on macOS, confirmed by the user on 2026-09-28. The trigger routing (Note
  Receiver plus a channel remap to the Trigger Channel) hasn't been confirmed specifically.
