//! The arpeggiator engine: note pool, step clock, and note output. It knows nothing about the
//! plugin host; `lib.rs` feeds it key presses and a clock one sample at a time.

use crate::pattern::{self, Mode};

pub const MAX_OCTAVES: usize = 4;
const MAX_KEYS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub key: u8,
    pub channel: u8,
    pub velocity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Out {
    On(Note),
    Off { key: u8, channel: u8 },
}

pub struct Settings {
    pub mode: Mode,
    /// Step length in quarter notes.
    pub step_beats: f64,
    pub octaves: usize,
    /// Note length as a fraction of the step, in `(0, 1]`.
    pub gate: f64,
    /// `None` plays each note at the velocity it was pressed with.
    pub velocity: Option<f32>,
    pub latch: bool,
    pub repeat_ends: bool,
    /// Start the pattern over on every chord change instead of continuing from the current step.
    pub restart_on_chord: bool,
}

pub struct Arp {
    /// Keys currently held down, sorted by pitch.
    held: Vec<Note>,
    /// Every key pressed since all keys were last up. This is the pool while latched.
    latched: Vec<Note>,
    pattern: Vec<usize>,
    /// The `(mode, m, repeat_ends)` that `pattern` was built for.
    built: Option<(Mode, usize, bool)>,
    /// Position in `pattern` of the next step.
    pos: usize,
    /// Whether the pool was non-empty on the previous tick.
    active: bool,
    chord_changed: bool,
    /// `(key, channel)` of the note that is on. The arp is monophonic.
    sounding: Option<(u8, u8)>,
    /// Samples left until the sounding note's gate closes.
    off_in: u32,
    /// Step index seen on the previous tick. A step fires on the first tick where this changes.
    last_step: Option<i64>,
    /// Position of the free-running clock used while the transport is stopped.
    free_beat: f64,
    playing: bool,
}

impl Default for Arp {
    fn default() -> Self {
        Self {
            held: Vec::with_capacity(MAX_KEYS),
            latched: Vec::with_capacity(MAX_KEYS),
            pattern: Vec::with_capacity(pattern::max_len(MAX_KEYS * MAX_OCTAVES)),
            built: None,
            pos: 0,
            active: false,
            chord_changed: false,
            sounding: None,
            off_in: 0,
            last_step: None,
            free_beat: 0.0,
            playing: false,
        }
    }
}

impl Arp {
    pub fn key_on(&mut self, note: Note) {
        if note.key as usize >= MAX_KEYS {
            return;
        }
        if self.held.is_empty() {
            self.latched.clear();
        }
        insert(&mut self.held, note);
        insert(&mut self.latched, note);
        self.chord_changed = true;
    }

    /// `None` releases every key.
    pub fn key_off(&mut self, key: Option<u8>, latch: bool) {
        match key {
            Some(key) => self.held.retain(|n| n.key != key),
            None => self.held.clear(),
        }
        self.chord_changed |= !latch;
    }

    /// Forgets all keys. The sounding note gets its note-off on the next tick, because the host
    /// gives no way to send events from outside of `process()`.
    pub fn reset(&mut self) {
        self.held.clear();
        self.latched.clear();
        self.active = false;
        self.last_step = None;
    }

    pub fn is_idle(&self) -> bool {
        !self.active && self.sounding.is_none()
    }

    /// Advances one sample. `beat` is the song position at this sample while the transport is
    /// playing, and `None` while it's stopped, in which case the arp free-runs from the tempo.
    pub fn tick(
        &mut self,
        beat: Option<f64>,
        beats_per_sample: f64,
        s: &Settings,
        emit: &mut impl FnMut(Out),
    ) {
        if beat.is_some() != self.playing {
            self.playing = beat.is_some();
            self.end_note(emit);
            self.pos = 0;
            self.free_beat = 0.0;
            // Pressing play fires the first step right away. Stopping waits out a step before
            // free-running, so notes the host releases just after the stop don't blip.
            self.last_step = if self.playing { None } else { Some(0) };
        }

        if self.sounding.is_some() {
            self.off_in = self.off_in.saturating_sub(1);
            if self.off_in == 0 {
                self.end_note(emit);
            }
        }

        let n = self.pool(s.latch).len();
        let chord_changed = std::mem::take(&mut self.chord_changed);
        if n == 0 {
            self.end_note(emit);
            self.active = false;
        } else if !self.active {
            self.active = true;
            self.pos = 0;
            if !self.playing {
                // Free-running starts on the key press.
                self.free_beat = 0.0;
                self.last_step = None;
            }
        } else if chord_changed && s.restart_on_chord {
            self.pos = 0;
        }

        let beat = beat.unwrap_or(self.free_beat);
        self.free_beat += beats_per_sample;
        let step = step_index(beat, s.step_beats);
        // Keep tracking steps with an empty pool, so that while the transport plays a new chord
        // waits for the next step on the grid.
        if self.last_step.replace(step) == Some(step) || n == 0 {
            return;
        }

        let m = n * s.octaves;
        if self.built != Some((s.mode, m, s.repeat_ends)) {
            pattern::fill(&mut self.pattern, s.mode, m, s.repeat_ends);
            self.built = Some((s.mode, m, s.repeat_ends));
            // Continue from the same step if the new pattern is long enough, else its last step.
            self.pos = self.pos.min(self.pattern.len() - 1);
        }
        let i = self.pattern[self.pos];
        self.pos = (self.pos + 1) % self.pattern.len();

        self.end_note(emit);
        let note = self.pool(s.latch)[i % n];
        let key = note.key as usize + 12 * (i / n);
        // Octaves past the top of the MIDI range are rests.
        if key < MAX_KEYS {
            let key = key as u8;
            emit(Out::On(Note {
                key,
                channel: note.channel,
                velocity: s.velocity.unwrap_or(note.velocity),
            }));
            self.sounding = Some((key, note.channel));
            self.off_in = gate_samples(s.gate, s.step_beats, beats_per_sample);
        }
    }

    fn pool(&self, latch: bool) -> &[Note] {
        if latch { &self.latched } else { &self.held }
    }

    fn end_note(&mut self, emit: &mut impl FnMut(Out)) {
        if let Some((key, channel)) = self.sounding.take() {
            emit(Out::Off { key, channel });
        }
    }
}

/// Keeps `pool` sorted by key with one entry per key. Never grows past `MAX_KEYS` entries, so it
/// never reallocates.
fn insert(pool: &mut Vec<Note>, note: Note) {
    match pool.binary_search_by_key(&note.key, |n| n.key) {
        Ok(i) => pool[i] = note,
        Err(i) => pool.insert(i, note),
    }
}

/// Index of the step containing `beat`. The epsilon puts a position that float error left just
/// short of a boundary (1.9999999 for 2.0) in the new step.
pub fn step_index(beat: f64, step_beats: f64) -> i64 {
    (beat / step_beats + 1e-9).floor() as i64
}

/// Gate length in samples. At least one, so a note-off never shares a sample with its note-on.
pub fn gate_samples(gate: f64, step_beats: f64, beats_per_sample: f64) -> u32 {
    ((gate * step_beats / beats_per_sample).round() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 BPM at 48 kHz: a 1/16 step is 6000 samples.
    const BPS: f64 = 120.0 / 60.0 / 48_000.0;
    const STEP: u64 = 6000;

    fn settings(mode: Mode) -> Settings {
        Settings {
            mode,
            step_beats: 0.25,
            octaves: 1,
            gate: 0.5,
            velocity: None,
            latch: false,
            repeat_ends: false,
            restart_on_chord: false,
        }
    }

    fn note(key: u8) -> Note {
        Note {
            key,
            channel: 0,
            velocity: 0.8,
        }
    }

    fn hold(arp: &mut Arp, keys: &[u8]) {
        for &key in keys {
            arp.key_on(note(key));
        }
    }

    /// Ticks samples `from..to` in host-sized blocks. `playing` puts beat 0 at sample 0, and each
    /// block starts from its own beat position the way a host reports it.
    fn run_blocks(
        arp: &mut Arp,
        s: &Settings,
        from: u64,
        to: u64,
        playing: bool,
        block: u64,
    ) -> Vec<(u64, Out)> {
        let mut out = Vec::new();
        for t in from..to {
            let block_start = t - t % block;
            let beat = playing.then(|| block_start as f64 * BPS + (t - block_start) as f64 * BPS);
            arp.tick(beat, BPS, s, &mut |o| out.push((t, o)));
        }
        out
    }

    fn run(arp: &mut Arp, s: &Settings, from: u64, to: u64, playing: bool) -> Vec<(u64, Out)> {
        run_blocks(arp, s, from, to, playing, 512)
    }

    fn ons(events: &[(u64, Out)]) -> Vec<(u64, u8)> {
        events
            .iter()
            .filter_map(|&(t, o)| match o {
                Out::On(n) => Some((t, n.key)),
                Out::Off { .. } => None,
            })
            .collect()
    }

    fn offs(events: &[(u64, Out)]) -> Vec<(u64, u8)> {
        events
            .iter()
            .filter_map(|&(t, o)| match o {
                Out::Off { key, .. } => Some((t, key)),
                Out::On(_) => None,
            })
            .collect()
    }

    /// Every note-on is followed by its note-off before the next note-on, and nothing is left on.
    fn assert_no_overlap(events: &[(u64, Out)]) {
        let mut on = None;
        for &(t, o) in events {
            match o {
                Out::On(n) => {
                    assert_eq!(on, None, "note-on at {t} while {on:?} is still on");
                    on = Some(n.key);
                }
                Out::Off { key, .. } => {
                    assert_eq!(on, Some(key), "stray note-off at {t}");
                    on = None;
                }
            }
        }
    }

    #[test]
    fn step_index_is_robust_at_boundaries() {
        assert_eq!(step_index(0.0, 0.25), 0);
        assert_eq!(step_index(0.2499, 0.25), 0);
        assert_eq!(step_index(0.25, 0.25), 1);
        assert_eq!(step_index(0.25 - 1e-12, 0.25), 1);
        assert_eq!(step_index(1.0, 1.0 / 3.0), 3);
        assert_eq!(step_index(-0.1, 0.25), -1);
    }

    #[test]
    fn gate_samples_scale_with_step_and_never_hit_zero() {
        assert_eq!(gate_samples(0.5, 0.25, BPS), 3000);
        assert_eq!(gate_samples(1.0, 1.0, BPS), 24_000);
        assert_eq!(gate_samples(0.001, 0.125, BPS), 3);
        assert_eq!(gate_samples(1e-9, 0.125, BPS), 1);
    }

    #[test]
    fn steps_land_on_the_grid_with_the_gate_in_between() {
        let s = settings(Mode::StairsUp);
        let mut arp = Arp::default();
        hold(&mut arp, &[67, 60, 64]);
        let events = run(&mut arp, &s, 0, 4 * STEP, true);
        assert_eq!(
            ons(&events),
            [(0, 60), (STEP, 67), (2 * STEP, 64), (3 * STEP, 60)]
        );
        assert_eq!(
            offs(&events),
            [(3000, 60), (9000, 67), (15000, 64), (21000, 60)]
        );
        assert_no_overlap(&events);
    }

    #[test]
    fn block_size_does_not_move_steps() {
        let s = Settings {
            step_beats: 1.0 / 6.0, // 1/16T: 4000 samples, 1/3 is not exact in binary
            ..settings(Mode::StairsUpDown)
        };
        let mut reference = None;
        for block in [1, 64, 441, 512, 4000, 4096] {
            let mut arp = Arp::default();
            hold(&mut arp, &[60, 62, 64, 65]);
            let events = run_blocks(&mut arp, &s, 0, 40 * 4000, true, block);
            let expected = reference.get_or_insert_with(|| events.clone());
            assert_eq!(&events, expected, "block size {block}");
        }
        let steps: Vec<u64> = ons(reference.as_ref().unwrap())
            .iter()
            .map(|&(t, _)| t)
            .collect();
        assert_eq!(steps, (0..40).map(|k| k * 4000).collect::<Vec<_>>());
    }

    #[test]
    fn full_gate_ends_each_note_right_before_the_next() {
        let s = Settings {
            gate: 1.0,
            ..settings(Mode::Up)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let events = run(&mut arp, &s, 0, 3 * STEP, true);
        assert_no_overlap(&events);
        assert_eq!(
            &events[1..3],
            [
                (
                    STEP,
                    Out::Off {
                        key: 60,
                        channel: 0
                    }
                ),
                (STEP, Out::On(note(64)))
            ]
        );
    }

    #[test]
    fn single_note_repeats() {
        let s = settings(Mode::StairsUpDown);
        let mut arp = Arp::default();
        hold(&mut arp, &[60]);
        let events = run(&mut arp, &s, 0, 4 * STEP, true);
        assert_eq!(
            ons(&events),
            [(0, 60), (STEP, 60), (2 * STEP, 60), (3 * STEP, 60)]
        );
    }

    #[test]
    fn octaves_stack_the_pool() {
        let s = Settings {
            octaves: 3,
            ..settings(Mode::Up)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let keys: Vec<u8> = ons(&run(&mut arp, &s, 0, 7 * STEP, true))
            .iter()
            .map(|&(_, k)| k)
            .collect();
        assert_eq!(keys, [60, 64, 72, 76, 84, 88, 60]);
    }

    #[test]
    fn octaves_past_the_midi_range_are_rests() {
        let s = Settings {
            octaves: 2,
            ..settings(Mode::Up)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[120]);
        let events = run(&mut arp, &s, 0, 3 * STEP, true);
        assert_eq!(ons(&events), [(0, 120), (2 * STEP, 120)]);
        assert_no_overlap(&events);
    }

    #[test]
    fn releasing_every_key_ends_the_note_at_once() {
        let s = settings(Mode::Up);
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let mut events = run(&mut arp, &s, 0, 1000, true);
        arp.key_off(Some(64), false);
        arp.key_off(Some(60), false);
        events.extend(run(&mut arp, &s, 1000, 3 * STEP, true));
        assert_eq!(offs(&events), [(1000, 60)]);
        assert_eq!(ons(&events), [(0, 60)]);
        assert!(arp.is_idle());
    }

    #[test]
    fn a_chord_pressed_mid_step_waits_for_the_grid_while_playing() {
        let s = settings(Mode::Up);
        let mut arp = Arp::default();
        run(&mut arp, &s, 0, 1000, true);
        hold(&mut arp, &[60]);
        assert_eq!(ons(&run(&mut arp, &s, 1000, 2 * STEP, true)), [(STEP, 60)]);
    }

    #[test]
    fn stopped_transport_free_runs_from_the_key_press() {
        let s = settings(Mode::Up);
        let mut arp = Arp::default();
        run(&mut arp, &s, 0, 500, false);
        hold(&mut arp, &[60, 64]);
        let events = run(&mut arp, &s, 500, 500 + 2 * STEP, false);
        assert_eq!(ons(&events), [(500, 60), (500 + STEP, 64)]);
    }

    #[test]
    fn stopping_the_transport_ends_the_note_then_free_runs_a_step_later() {
        let s = settings(Mode::Up);
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let mut events = run(&mut arp, &s, 0, 1000, true);
        events.extend(run(&mut arp, &s, 1000, 1000 + 2 * STEP, false));
        assert_eq!(offs(&events)[0], (1000, 60));
        assert_eq!(ons(&events), [(0, 60), (1000 + STEP, 60)]);
        assert_no_overlap(&events);
    }

    #[test]
    fn starting_the_transport_restarts_the_pattern() {
        let s = settings(Mode::Up);
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64, 67]);
        let mut events = run(&mut arp, &s, 0, STEP + 10, false);
        events.extend(run(&mut arp, &s, STEP + 10, 2 * STEP, true));
        assert_eq!(ons(&events), [(0, 60), (STEP, 64), (STEP + 10, 60)]);
        assert_eq!(
            offs(&events),
            [(3000, 60), (STEP + 10, 64), (STEP + 3010, 60)]
        );
        assert_no_overlap(&events);
    }

    #[test]
    fn reset_ends_the_note_on_the_next_tick() {
        let s = settings(Mode::Up);
        let mut arp = Arp::default();
        hold(&mut arp, &[60]);
        run(&mut arp, &s, 0, 100, true);
        arp.reset();
        let events = run(&mut arp, &s, 100, 2 * STEP, true);
        assert_eq!(
            events,
            [(
                100,
                Out::Off {
                    key: 60,
                    channel: 0
                }
            )]
        );
    }

    #[test]
    fn chord_changes_continue_from_the_clamped_step() {
        let s = settings(Mode::StairsUp);
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 62, 64, 65, 67]); // m = 5: 0 2 1 3 2 4 3
        let mut events = run(&mut arp, &s, 0, 6 * STEP, true); // plays 0 2 1 3 2 4
        arp.key_off(Some(65), false);
        arp.key_off(Some(67), false); // m = 3: 0 2 1, step 6 clamps to 2
        events.extend(run(&mut arp, &s, 6 * STEP, 8 * STEP, true));
        let keys: Vec<u8> = ons(&events).iter().map(|&(_, k)| k).collect();
        assert_eq!(keys, [60, 64, 62, 65, 64, 67, 62, 60]);
    }

    #[test]
    fn restart_on_chord_goes_back_to_the_first_step() {
        let s = Settings {
            restart_on_chord: true,
            ..settings(Mode::Up)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64, 67]);
        let mut events = run(&mut arp, &s, 0, 2 * STEP, true);
        hold(&mut arp, &[72]);
        events.extend(run(&mut arp, &s, 2 * STEP, 3 * STEP, true));
        let keys: Vec<u8> = ons(&events).iter().map(|&(_, k)| k).collect();
        assert_eq!(keys, [60, 64, 60]);
    }

    #[test]
    fn latch_holds_the_chord_until_a_new_one_is_played() {
        let s = Settings {
            latch: true,
            ..settings(Mode::Up)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let mut events = run(&mut arp, &s, 0, 10, true);
        arp.key_off(Some(60), true);
        arp.key_off(Some(64), true);
        events.extend(run(&mut arp, &s, 10, 3 * STEP, true));
        hold(&mut arp, &[50]);
        events.extend(run(&mut arp, &s, 3 * STEP, 5 * STEP, true));
        let keys: Vec<u8> = ons(&events).iter().map(|&(_, k)| k).collect();
        assert_eq!(keys, [60, 64, 60, 50, 50]);

        // Turning latch off with no keys down stops the arp.
        arp.key_off(Some(50), true);
        let events = run(&mut arp, &settings(Mode::Up), 5 * STEP, 6 * STEP, true);
        assert!(ons(&events).is_empty());
        assert!(arp.is_idle());
    }

    #[test]
    fn fixed_velocity_overrides_the_played_one() {
        let s = Settings {
            velocity: Some(0.5),
            ..settings(Mode::Up)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60]);
        let events = run(&mut arp, &s, 0, 1, true);
        assert_eq!(
            events,
            [(
                0,
                Out::On(Note {
                    velocity: 0.5,
                    ..note(60)
                })
            )]
        );
    }
}
