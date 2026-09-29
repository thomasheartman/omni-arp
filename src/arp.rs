//! The arpeggiator engine: note pool, step clock, and note output. It knows nothing about the
//! plugin host; `lib.rs` feeds it key presses, trigger notes and a clock one sample at a time.

use crate::pattern::{self, First, OctaveBehavior, Pair, Shape, Spec};
use std::hash::{BuildHasher, RandomState};

/// Octaves the range can reach below and above the held notes.
pub const MAX_OCTAVE_SHIFT: usize = 3;
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Velocity {
    /// Each held note's own velocity, or the trigger's when a trigger note steps the pattern.
    AsPlayed,
    Fixed(f32),
}

pub struct Settings {
    pub pattern: Spec,
    /// Step length in quarter notes.
    pub step_beats: f64,
    pub octaves_down: usize,
    pub octaves_up: usize,
    /// Steps before the pattern starts over, or 0 to play whole cycles.
    pub length: usize,
    /// Times each step plays, at least 1.
    pub repeats: usize,
    /// Notes per step: the pattern's note on top, then the next held notes below it.
    pub notes: usize,
    /// Velocity of the notes below the top one, as a fraction of the top note's.
    pub chord_velocity: f32,
    /// Note length as a fraction of the step, in `[0, 2]`.
    pub note_length: f64,
    pub velocity: Velocity,
    /// Steps fire on trigger notes instead of the tempo grid.
    pub triggered: bool,
    pub latch: bool,
    /// Start the pattern over on every chord change instead of continuing from the current step.
    pub restart_on_chord: bool,
}

/// When a sounding note ends.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Until {
    /// After this many more samples.
    Samples(u32),
    /// With the note-off of the trigger note that started it.
    Release { key: u8, channel: u8 },
}

#[derive(Debug, Clone, Copy)]
struct Sounding {
    key: u8,
    channel: u8,
    until: Until,
}

pub struct Arp {
    /// Keys currently held down, sorted by pitch.
    held: Vec<Note>,
    /// Every key pressed since all keys were last up. This is the pool while latched.
    latched: Vec<Note>,
    /// The keys the pattern's indices point at: the pool stacked over the octave range, each as
    /// `(key, index into the pool)`.
    range: Vec<(i32, usize)>,
    pattern: Vec<usize>,
    /// The `(spec, notes, octaves)` that `pattern` was built for.
    built: Option<(Spec, usize, usize)>,
    /// Position in `pattern` of the next step.
    pos: usize,
    /// Steps played from the pattern entry at `pos`: each plays `repeats` times, and twice that
    /// with an echo.
    sub: usize,
    /// The key of the lead note played last, so a new shuffle doesn't start on it.
    last_key: Option<i32>,
    /// Steps played since the pattern started, for Length.
    played: usize,
    /// Whether the pool was non-empty on the previous tick.
    active: bool,
    chord_changed: bool,
    /// Notes that are on, at most one per key.
    sounding: Vec<Sounding>,
    /// Step index seen on the previous tick. A step fires on the first tick where this changes.
    last_step: Option<i64>,
    /// Position of the free-running clock used while the transport is stopped.
    free_beat: f64,
    playing: bool,
    /// Shuffle's xorshift state. Never zero.
    rng: u64,
}

impl Default for Arp {
    fn default() -> Self {
        let max_octaves = 2 * MAX_OCTAVE_SHIFT + 1;
        Self {
            held: Vec::with_capacity(MAX_KEYS),
            latched: Vec::with_capacity(MAX_KEYS),
            range: Vec::with_capacity(MAX_KEYS * max_octaves),
            pattern: Vec::with_capacity(pattern::max_len(MAX_KEYS * max_octaves)),
            built: None,
            pos: 0,
            sub: 0,
            last_key: None,
            played: 0,
            active: false,
            chord_changed: false,
            sounding: Vec::with_capacity(MAX_KEYS),
            last_step: None,
            free_beat: 0.0,
            playing: false,
            // Seeded per instance, so two arps shuffling don't play the same orders.
            rng: RandomState::new().hash_one(0) | 1,
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

    /// `None` matches any key or channel.
    pub fn key_off(&mut self, key: Option<u8>, channel: Option<u8>, latch: bool) {
        let before = self.held.len();
        self.held.retain(|n| {
            !(key.is_none_or(|k| k == n.key) && channel.is_none_or(|c| c == n.channel))
        });
        self.chord_changed |= !latch && self.held.len() != before;
    }

    /// Plays the next step now, for Advance: Trigger. Its notes end with the trigger's note-off.
    pub fn trigger(&mut self, trigger: Note, s: &Settings, emit: &mut impl FnMut(Out)) {
        let n = self.sync(s, emit);
        if n > 0 {
            let until = Until::Release {
                key: trigger.key,
                channel: trigger.channel,
            };
            self.step(n, s, until, Some(trigger.velocity), emit);
        }
    }

    /// Ends the notes a trigger note started. `None` matches any key or channel. Call this for
    /// every note-off: a held trigger outlives changes to Advance and Trigger Channel.
    pub fn release(&mut self, key: Option<u8>, channel: Option<u8>, emit: &mut impl FnMut(Out)) {
        self.end_where(emit, |note| match note.until {
            Until::Release { key: k, channel: c } => {
                key.is_none_or(|key| key == k) && channel.is_none_or(|channel| channel == c)
            }
            Until::Samples(_) => false,
        });
    }

    /// Forgets all keys. Sounding notes get their note-offs on the next tick, because the host
    /// gives no way to send events from outside of `process()`.
    pub fn reset(&mut self) {
        self.held.clear();
        self.latched.clear();
        self.active = false;
        self.last_step = None;
        self.restart();
    }

    pub fn is_idle(&self) -> bool {
        !self.active && self.sounding.is_empty()
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
            self.end_where(emit, |_| true);
            self.restart();
            self.free_beat = 0.0;
            // Pressing play fires the first step right away. Stopping waits out a step before
            // free-running, so notes the host releases just after the stop don't blip.
            self.last_step = if self.playing { None } else { Some(0) };
        }

        self.sounding.retain_mut(|note| {
            if let Until::Samples(left) = &mut note.until {
                *left = left.saturating_sub(1);
                if *left == 0 {
                    emit(Out::Off {
                        key: note.key,
                        channel: note.channel,
                    });
                    return false;
                }
            }
            true
        });

        let n = self.sync(s, emit);
        let beat = beat.unwrap_or(self.free_beat);
        self.free_beat += beats_per_sample;
        let step = step_index(beat, s.step_beats);
        // Keep tracking steps with an empty pool, so that while the transport plays a new chord
        // waits for the next step on the grid.
        if self.last_step.replace(step) == Some(step) || n == 0 || s.triggered {
            return;
        }
        let length = note_samples(s.note_length, s.step_beats, beats_per_sample);
        self.step(n, s, Until::Samples(length), None, emit);
    }

    /// Catches up with pool changes: everything ends when the pool empties, and the pattern
    /// starts over when it fills again or, if asked, on a chord change. Returns the pool size.
    fn sync(&mut self, s: &Settings, emit: &mut impl FnMut(Out)) -> usize {
        let n = if s.latch { &self.latched } else { &self.held }.len();
        let chord_changed = std::mem::take(&mut self.chord_changed);
        if n == 0 {
            self.end_where(emit, |_| true);
            self.active = false;
        } else if !self.active {
            self.active = true;
            self.restart();
            if !self.playing {
                // Free-running starts on the key press.
                self.free_beat = 0.0;
                self.last_step = None;
            }
        } else if chord_changed && s.restart_on_chord {
            self.restart();
        }
        n
    }

    fn step(
        &mut self,
        n: usize,
        s: &Settings,
        until: Until,
        trigger_velocity: Option<f32>,
        emit: &mut impl FnMut(Out),
    ) {
        let octaves = s.octaves_down + 1 + s.octaves_up;
        let thin = s.pattern.octave_behavior == OctaveBehavior::Thin;
        let pool = if s.latch { &self.latched } else { &self.held };
        self.range.clear();
        for octave in 0..octaves {
            let shift = 12 * (octave as i32 - s.octaves_down as i32);
            for (k, note) in pool.iter().enumerate() {
                let key = note.key as i32 + shift;
                // Thin walks one line of notes, so when the chord's top note is its bottom note
                // an octave up, the next copy's bottom note would repeat it. It plays once.
                if !(thin && self.range.last().is_some_and(|&(last, _)| last == key)) {
                    self.range.push((key, k));
                }
            }
        }
        // Thin walks the range as one line; the others walk the held notes and handle the
        // octaves themselves.
        let (notes, copies) = if thin {
            (self.range.len(), 1)
        } else {
            (n, octaves)
        };

        if s.length > 0 && self.played >= s.length {
            self.restart();
        }
        let rebuild = self.built != Some((s.pattern, notes, copies));
        let reshuffle = s.pattern.shape == Shape::Shuffle && self.pos == 0 && self.sub == 0;
        if rebuild || reshuffle {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            pattern::fill(&mut self.pattern, &s.pattern, notes, copies, self.rng);
            self.built = Some((s.pattern, notes, copies));
            if rebuild {
                // Continue from the same step if the new pattern is long enough, else its last.
                self.pos = self.pos.min(self.pattern.len() - 1);
                self.sub = 0;
            }
            // A new shuffle doesn't start on the note that just played. Keys, not indices: the
            // pool may have changed since.
            let starts_on_last = self.last_key == Some(self.range[self.pattern[0]].0);
            if s.pattern.shape == Shape::Shuffle && self.pattern.len() > 1 && starts_on_last {
                self.pattern.swap(0, 1);
            }
        }

        let echo = match s.pattern.pair {
            Pair::EchoBelow => Some(-12),
            Pair::EchoAbove => Some(12),
            _ => None,
        };
        let parts = if echo.is_some() { 2 } else { 1 };
        // The echo follows its step, or comes first with First: Pair.
        let second_part = self.sub / s.repeats == 1;
        let echo_shift = match echo {
            Some(shift) if second_part != (s.pattern.first == First::Pair) => shift,
            _ => 0,
        };
        let i = self.pattern[self.pos];
        self.last_key = Some(self.range[i].0);
        self.played += 1;
        self.sub += 1;
        if self.sub >= parts * s.repeats {
            self.sub = 0;
            self.pos = (self.pos + 1) % self.pattern.len();
        }

        let pool = if s.latch { &self.latched } else { &self.held };
        let (key, k) = self.range[i];
        let lead = pool[k];
        let top = key + echo_shift;
        let shift = top - lead.key as i32;
        // Each held note moves into the pattern note's octave, then down below it: the inversion
        // with the pattern's note on top.
        let voiced = |note: &Note| {
            let mut key = note.key as i32 + shift;
            while key > top || (key == top && note.key != lead.key) {
                key -= 12;
            }
            key
        };
        let velocity = match s.velocity {
            Velocity::AsPlayed => trigger_velocity.unwrap_or(lead.velocity),
            Velocity::Fixed(velocity) => velocity,
        };
        let mut ceiling = top + 1;
        for voice in 0..s.notes.min(n) {
            let Some((key, note)) = pool
                .iter()
                .map(|note| (voiced(note), note))
                .filter(|&(key, _)| key < ceiling)
                .max_by_key(|&(key, _)| key)
            else {
                break;
            };
            ceiling = key;
            // Octaves past either end of the MIDI range are rests.
            if (0..MAX_KEYS as i32).contains(&key) {
                // The top note is the accent; the rest follow it, whatever their own velocity.
                let velocity = if voice == 0 {
                    velocity
                } else {
                    velocity * s.chord_velocity
                };
                let note = Note {
                    key: key as u8,
                    channel: note.channel,
                    velocity,
                };
                start(&mut self.sounding, emit, note, until);
            }
        }
    }

    fn restart(&mut self) {
        self.pos = 0;
        self.sub = 0;
        self.played = 0;
    }

    fn end_where(&mut self, emit: &mut impl FnMut(Out), ends: impl Fn(&Sounding) -> bool) {
        self.sounding.retain(|note| {
            if !ends(note) {
                return true;
            }
            emit(Out::Off {
                key: note.key,
                channel: note.channel,
            });
            false
        });
    }
}

/// Starts a note, first ending the same key if it's still on: MIDI can't hold a key twice, and
/// the old note-off would cut the new note short.
fn start(sounding: &mut Vec<Sounding>, emit: &mut impl FnMut(Out), note: Note, until: Until) {
    if let Some(k) = sounding.iter().position(|s| s.key == note.key) {
        let old = sounding.swap_remove(k);
        emit(Out::Off {
            key: old.key,
            channel: old.channel,
        });
    }
    emit(Out::On(note));
    sounding.push(Sounding {
        key: note.key,
        channel: note.channel,
        until,
    });
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

/// Note length in samples. At least one, so a note-off never shares a sample with its note-on.
pub fn note_samples(fraction: f64, step_beats: f64, beats_per_sample: f64) -> u32 {
    ((fraction * step_beats / beats_per_sample).round() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::{Direction, Edge, OctaveBehavior, Start};

    /// 120 BPM at 48 kHz: a 1/16 step is 6000 samples.
    const BPS: f64 = 120.0 / 60.0 / 48_000.0;
    const STEP: u64 = 6000;

    fn settings(shape: Shape) -> Settings {
        Settings {
            pattern: Spec {
                shape,
                direction: Direction::Up,
                start: Start::Outside,
                edge: Edge::Restart,
                pair: Pair::Off,
                first: First::Lead,
                repeat_ends: false,
                octave_behavior: OctaveBehavior::Thin,
            },
            step_beats: 0.25,
            octaves_down: 0,
            octaves_up: 0,
            length: 0,
            repeats: 1,
            notes: 1,
            chord_velocity: 1.0,
            note_length: 0.5,
            velocity: Velocity::AsPlayed,
            triggered: false,
            latch: false,
            restart_on_chord: false,
        }
    }

    fn with_edge(shape: Shape, edge: Edge) -> Settings {
        let mut s = settings(shape);
        s.pattern.edge = edge;
        s
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

    fn keys(events: &[(u64, Out)]) -> Vec<u8> {
        ons(events).iter().map(|&(_, k)| k).collect()
    }

    /// The keys each step starts, in the order they start.
    fn chords(events: &[(u64, Out)]) -> Vec<Vec<u8>> {
        let mut chords: Vec<(u64, Vec<u8>)> = Vec::new();
        for (t, key) in ons(events) {
            match chords.last_mut() {
                Some((start, keys)) if *start == t => keys.push(key),
                _ => chords.push((t, vec![key])),
            }
        }
        chords.into_iter().map(|(_, keys)| keys).collect()
    }

    /// Every note from a step ends before the next step's notes start, and every note-off matches
    /// a sounding note.
    fn assert_no_overlap(events: &[(u64, Out)]) {
        let mut on: Vec<(u64, u8)> = Vec::new();
        for &(t, o) in events {
            match o {
                Out::On(n) => {
                    assert!(
                        on.iter().all(|&(start, key)| start == t && key != n.key),
                        "note-on at {t} while {on:?} are still on"
                    );
                    on.push((t, n.key));
                }
                Out::Off { key, .. } => {
                    let i = on.iter().position(|&(_, k)| k == key);
                    on.remove(i.unwrap_or_else(|| panic!("stray note-off at {t}")));
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
    fn note_samples_scale_with_step_and_never_hit_zero() {
        assert_eq!(note_samples(0.5, 0.25, BPS), 3000);
        assert_eq!(note_samples(1.0, 1.0, BPS), 24_000);
        assert_eq!(note_samples(2.0, 0.25, BPS), 12_000);
        assert_eq!(note_samples(0.001, 0.125, BPS), 3);
        assert_eq!(note_samples(0.0, 0.125, BPS), 1);
    }

    #[test]
    fn steps_land_on_the_grid_with_the_note_length_in_between() {
        let s = settings(Shape::Stairs);
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
            ..with_edge(Shape::Stairs, Edge::Reverse)
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
    fn full_length_ends_each_note_right_before_the_next() {
        let s = Settings {
            note_length: 1.0,
            ..settings(Shape::Straight)
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
    fn longer_notes_overlap_the_next_step() {
        let s = Settings {
            note_length: 1.5,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let events = run(&mut arp, &s, 0, 2 * STEP + 1, true);
        assert_eq!(
            events,
            [
                (0, Out::On(note(60))),
                (STEP, Out::On(note(64))),
                (
                    9000,
                    Out::Off {
                        key: 60,
                        channel: 0
                    }
                ),
                (2 * STEP, Out::On(note(60))),
            ]
        );
    }

    #[test]
    fn a_key_that_comes_round_while_still_on_ends_first() {
        let s = Settings {
            note_length: 2.0,
            repeats: 2,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60]);
        let events = run(&mut arp, &s, 0, STEP + 1, true);
        assert_eq!(
            events,
            [
                (0, Out::On(note(60))),
                (
                    STEP,
                    Out::Off {
                        key: 60,
                        channel: 0
                    }
                ),
                (STEP, Out::On(note(60))),
            ]
        );
    }

    #[test]
    fn repeats_play_each_step_several_times() {
        let s = Settings {
            repeats: 4,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let events = run(&mut arp, &s, 0, 9 * STEP, true);
        assert_eq!(keys(&events), [60, 60, 60, 60, 64, 64, 64, 64, 60]);
    }

    #[test]
    fn echoes_repeat_each_step_an_octave_away() {
        let echo = |pair, first, repeats| {
            let mut s = Settings {
                repeats,
                ..settings(Shape::Straight)
            };
            s.pattern.pair = pair;
            s.pattern.first = first;
            s
        };
        let play = |s: &Settings, steps| {
            let mut arp = Arp::default();
            hold(&mut arp, &[60, 64]);
            keys(&run(&mut arp, s, 0, steps * STEP, true))
        };
        let s = echo(Pair::EchoBelow, First::Lead, 1);
        assert_eq!(play(&s, 5), [60, 48, 64, 52, 60]);
        // First: Pair puts the echo ahead of its note.
        let s = echo(Pair::EchoAbove, First::Pair, 1);
        assert_eq!(play(&s, 4), [72, 60, 76, 64]);
        let s = echo(Pair::EchoBelow, First::Lead, 2);
        assert_eq!(play(&s, 8), [60, 60, 48, 48, 64, 64, 52, 52]);

        // With Notes above 1 the whole chord echoes.
        let s = Settings {
            notes: usize::MAX,
            ..echo(Pair::EchoBelow, First::Lead, 1)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let events = run(&mut arp, &s, 0, 2 * STEP, true);
        assert_eq!(chords(&events), [vec![60, 52], vec![48, 40]]);
    }

    #[test]
    fn shuffle_plays_every_note_once_per_cycle_in_new_orders() {
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 62, 64, 65, 67]);
        let played = keys(&run(
            &mut arp,
            &settings(Shape::Shuffle),
            0,
            40 * STEP,
            true,
        ));
        for cycle in played.chunks(5) {
            let mut sorted = cycle.to_vec();
            sorted.sort();
            assert_eq!(sorted, [60, 62, 64, 65, 67], "{played:?}");
        }
        // Includes the seams between cycles.
        assert!(played.windows(2).all(|w| w[0] != w[1]), "{played:?}");
        // Eight identical cycles in a row would be a (1/120)^7 fluke.
        assert!(
            played.chunks(5).any(|c| c != &played[..5]),
            "the order never changes"
        );
    }

    #[test]
    fn length_starts_the_pattern_over() {
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 62, 64, 65]);
        let s = Settings {
            length: 3,
            ..settings(Shape::Straight)
        };
        let events = run(&mut arp, &s, 0, 7 * STEP, true);
        assert_eq!(keys(&events), [60, 62, 64, 60, 62, 64, 60]);

        // Longer than the cycle: the cycle keeps going until Length steps have played.
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 62, 64]);
        let s = Settings {
            length: 4,
            ..settings(Shape::Straight)
        };
        let events = run(&mut arp, &s, 0, 8 * STEP, true);
        assert_eq!(keys(&events), [60, 62, 64, 60, 60, 62, 64, 60]);
    }

    #[test]
    fn single_note_repeats() {
        let s = with_edge(Shape::Stairs, Edge::Reverse);
        let mut arp = Arp::default();
        hold(&mut arp, &[60]);
        let events = run(&mut arp, &s, 0, 4 * STEP, true);
        assert_eq!(
            ons(&events),
            [(0, 60), (STEP, 60), (2 * STEP, 60), (3 * STEP, 60)]
        );
    }

    #[test]
    fn octaves_extend_the_range_both_ways() {
        let s = Settings {
            octaves_up: 2,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let events = run(&mut arp, &s, 0, 7 * STEP, true);
        assert_eq!(keys(&events), [60, 64, 72, 76, 84, 88, 60]);

        let s = Settings {
            octaves_down: 1,
            octaves_up: 1,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let events = run(&mut arp, &s, 0, 6 * STEP, true);
        assert_eq!(keys(&events), [48, 52, 60, 64, 72, 76]);
    }

    #[test]
    fn octave_copies_never_repeat_a_note_back_to_back() {
        let play = |shape, octave_behavior, held: &[u8], steps| {
            let mut s = Settings {
                octaves_up: 1,
                ..settings(shape)
            };
            s.pattern.octave_behavior = octave_behavior;
            let mut arp = Arp::default();
            hold(&mut arp, held);
            keys(&run(&mut arp, &s, 0, steps * STEP, true))
        };
        // C E G C': the next octave's C' is the chord's own top note, so it plays once.
        assert_eq!(
            play(Shape::Straight, OctaveBehavior::Thin, &[60, 64, 67, 72], 7),
            [60, 64, 67, 72, 76, 79, 84]
        );
        assert_eq!(
            play(
                Shape::GroupsOfThree,
                OctaveBehavior::Thin,
                &[60, 64, 67, 72],
                15
            ),
            [60, 64, 67, 64, 67, 72, 67, 72, 76, 72, 76, 79, 76, 79, 84]
        );
        // C E C' D': no neighbours repeat, so the copies stack as they are.
        assert_eq!(
            play(Shape::Straight, OctaveBehavior::Thin, &[60, 64, 72, 74], 8),
            [60, 64, 72, 74, 72, 76, 84, 86]
        );
        // 1 by 1 plays whole copies of the chord, one octave after the other.
        assert_eq!(
            play(
                Shape::Straight,
                OctaveBehavior::OneByOne,
                &[60, 64, 67, 72],
                8
            ),
            [60, 64, 67, 72, 72, 76, 79, 84]
        );
    }

    #[test]
    fn octaves_past_the_midi_range_are_rests() {
        let s = Settings {
            octaves_up: 1,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[120]);
        let events = run(&mut arp, &s, 0, 3 * STEP, true);
        assert_eq!(ons(&events), [(0, 120), (2 * STEP, 120)]);
        assert_no_overlap(&events);
    }

    #[test]
    fn notes_play_the_inversion_with_the_pattern_note_on_top() {
        let s = Settings {
            notes: usize::MAX,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64, 67]);
        let events = run(&mut arp, &s, 0, 3 * STEP, true);
        assert_eq!(
            chords(&events),
            [vec![60, 55, 52], vec![64, 60, 55], vec![67, 64, 60]]
        );
        assert_no_overlap(&events);

        // Two notes: the pattern's note and the next one down.
        let s = Settings { notes: 2, ..s };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64, 67, 72]);
        let events = run(&mut arp, &s, 0, 4 * STEP, true);
        assert_eq!(
            chords(&events),
            [vec![60, 55], vec![64, 60], vec![67, 64], vec![72, 67]]
        );
    }

    #[test]
    fn notes_below_the_top_play_at_chord_velocity() {
        let s = Settings {
            notes: usize::MAX,
            chord_velocity: 0.5,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        // Softer on top than below: the lower notes follow the top note, not their own velocity.
        arp.key_on(Note {
            velocity: 0.8,
            ..note(60)
        });
        arp.key_on(Note {
            velocity: 1.0,
            ..note(64)
        });
        arp.key_on(Note {
            velocity: 0.2,
            ..note(67)
        });
        let velocities: Vec<(u8, f32)> = run(&mut arp, &s, 0, 1, true)
            .into_iter()
            .filter_map(|(_, o)| match o {
                Out::On(n) => Some((n.key, n.velocity)),
                Out::Off { .. } => None,
            })
            .collect();
        assert_eq!(velocities, [(60, 0.8), (55, 0.4), (52, 0.4)]);
    }

    #[test]
    fn releasing_every_key_ends_the_note_at_once() {
        let s = Settings {
            notes: usize::MAX,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let mut events = run(&mut arp, &s, 0, 1000, true);
        arp.key_off(Some(64), None, false);
        arp.key_off(Some(60), None, false);
        events.extend(run(&mut arp, &s, 1000, 3 * STEP, true));
        assert_eq!(offs(&events), [(1000, 60), (1000, 52)]);
        assert!(arp.is_idle());
    }

    #[test]
    fn note_offs_only_release_the_matching_channel() {
        let s = settings(Shape::Straight);
        let mut arp = Arp::default();
        hold(&mut arp, &[60]);
        arp.key_off(Some(60), Some(15), false);
        assert_eq!(keys(&run(&mut arp, &s, 0, 1, true)), [60]);
        arp.key_off(Some(60), Some(0), false);
        run(&mut arp, &s, 1, 2, true);
        assert!(arp.is_idle());
    }

    #[test]
    fn a_chord_pressed_mid_step_waits_for_the_grid_while_playing() {
        let s = settings(Shape::Straight);
        let mut arp = Arp::default();
        run(&mut arp, &s, 0, 1000, true);
        hold(&mut arp, &[60]);
        assert_eq!(ons(&run(&mut arp, &s, 1000, 2 * STEP, true)), [(STEP, 60)]);
    }

    #[test]
    fn a_chord_played_by_hand_still_runs_the_pattern_from_the_bottom() {
        // Transport stopped, keys a few ms apart with E first, as when playing a chord by hand.
        let s = with_edge(Shape::GroupsOfThree, Edge::Reverse);
        let mut arp = Arp::default();
        let mut events = Vec::new();
        let mut from = 0;
        for (t, key) in [(0, 64), (150, 60), (300, 67), (450, 72)] {
            events.extend(run(&mut arp, &s, from, t, false));
            arp.key_on(note(key));
            from = t;
        }
        events.extend(run(&mut arp, &s, from, 21 * STEP, false));
        let played = keys(&events);
        // E sounds alone on the key press; after that the full chord's cycle repeats unchanged.
        let cycle = [60, 64, 67, 64, 67, 72, 67, 64, 67, 64];
        assert_eq!(played[1..11], cycle, "{played:?}");
        assert_eq!(played[11..21], cycle, "{played:?}");
    }

    #[test]
    fn stopped_transport_free_runs_from_the_key_press() {
        let s = settings(Shape::Straight);
        let mut arp = Arp::default();
        run(&mut arp, &s, 0, 500, false);
        hold(&mut arp, &[60, 64]);
        let events = run(&mut arp, &s, 500, 500 + 2 * STEP, false);
        assert_eq!(ons(&events), [(500, 60), (500 + STEP, 64)]);
    }

    #[test]
    fn stopping_the_transport_ends_the_note_then_free_runs_a_step_later() {
        let s = settings(Shape::Straight);
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
        let s = settings(Shape::Straight);
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
        let s = settings(Shape::Straight);
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
        let s = settings(Shape::Stairs);
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 62, 64, 65, 67]); // m = 5: 0 2 1 3 2 4
        let mut events = run(&mut arp, &s, 0, 5 * STEP, true); // plays 0 2 1 3 2
        arp.key_off(Some(65), None, false);
        arp.key_off(Some(67), None, false); // m = 3: 0 2 1, step 5 clamps to 2
        events.extend(run(&mut arp, &s, 5 * STEP, 7 * STEP, true));
        assert_eq!(keys(&events), [60, 64, 62, 65, 64, 62, 60]);
    }

    #[test]
    fn restart_on_chord_goes_back_to_the_first_step() {
        let s = Settings {
            restart_on_chord: true,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64, 67]);
        let mut events = run(&mut arp, &s, 0, 2 * STEP, true);
        hold(&mut arp, &[72]);
        events.extend(run(&mut arp, &s, 2 * STEP, 3 * STEP, true));
        assert_eq!(keys(&events), [60, 64, 60]);
    }

    #[test]
    fn latch_holds_the_chord_until_a_new_one_is_played() {
        let s = Settings {
            latch: true,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let mut events = run(&mut arp, &s, 0, 10, true);
        arp.key_off(Some(60), None, true);
        arp.key_off(Some(64), None, true);
        events.extend(run(&mut arp, &s, 10, 3 * STEP, true));
        hold(&mut arp, &[50]);
        events.extend(run(&mut arp, &s, 3 * STEP, 5 * STEP, true));
        assert_eq!(keys(&events), [60, 64, 60, 50, 50]);

        // Turning latch off with no keys down stops the arp.
        arp.key_off(Some(50), None, true);
        let events = run(
            &mut arp,
            &settings(Shape::Straight),
            5 * STEP,
            6 * STEP,
            true,
        );
        assert!(ons(&events).is_empty());
        assert!(arp.is_idle());
    }

    #[test]
    fn triggers_step_the_pattern_and_end_its_notes() {
        let s = Settings {
            triggered: true,
            ..settings(Shape::Straight)
        };
        let mut arp = Arp::default();
        hold(&mut arp, &[60, 64]);
        let hit = |velocity| Note {
            key: 36,
            channel: 15,
            velocity,
        };
        let mut events = Vec::new();
        // The tempo grid does nothing.
        events.extend(run(&mut arp, &s, 0, 2 * STEP, true));
        arp.trigger(hit(0.3), &s, &mut |o| events.push((20_000, o)));
        events.extend(run(&mut arp, &s, 2 * STEP, 4 * STEP, true));
        arp.release(Some(36), Some(15), &mut |o| events.push((30_000, o)));
        arp.trigger(hit(1.0), &s, &mut |o| events.push((40_000, o)));
        assert_eq!(
            events,
            [
                (
                    20_000,
                    Out::On(Note {
                        velocity: 0.3,
                        ..note(60)
                    })
                ),
                (
                    30_000,
                    Out::Off {
                        key: 60,
                        channel: 0
                    }
                ),
                (
                    40_000,
                    Out::On(Note {
                        velocity: 1.0,
                        ..note(64)
                    })
                ),
            ]
        );

        // A trigger still ends its notes after Advance changes back to Tempo.
        let mut events = Vec::new();
        arp.release(Some(36), Some(15), &mut |o| events.push(o));
        assert_eq!(
            events,
            [Out::Off {
                key: 64,
                channel: 0
            }]
        );
    }

    #[test]
    fn fixed_velocity_overrides_the_played_one() {
        let s = Settings {
            velocity: Velocity::Fixed(0.5),
            ..settings(Shape::Straight)
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

    /// nice-plug's `assert_process_allocs` allocator aborts the test binary if anything inside
    /// `assert_no_alloc` allocates.
    #[test]
    fn the_realtime_path_never_allocates() {
        let mut arp = Arp::default();
        let mut events = 0;
        for shape in [
            Shape::Straight,
            Shape::Stairs,
            Shape::GroupsOfThree,
            Shape::Shuffle,
        ] {
            for edge in [Edge::Restart, Edge::Reverse, Edge::Wrap] {
                for pair in [
                    Pair::Off,
                    Pair::Mirror,
                    Pair::Low,
                    Pair::High,
                    Pair::EchoBelow,
                    Pair::EchoAbove,
                ] {
                    for (k, octave_behavior) in [
                        OctaveBehavior::Thin,
                        OctaveBehavior::OneByOne,
                        OctaveBehavior::Alt,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let mut s = Settings {
                            octaves_down: MAX_OCTAVE_SHIFT,
                            octaves_up: MAX_OCTAVE_SHIFT,
                            length: 5,
                            repeats: 1 + k,
                            notes: [1, 3, usize::MAX][k],
                            note_length: 2.0,
                            latch: k == 1,
                            ..settings(shape)
                        };
                        s.pattern = Spec {
                            edge,
                            pair,
                            octave_behavior,
                            start: [Start::Outside, Start::Middle][k % 2],
                            first: [First::Lead, First::Pair][k % 2],
                            repeat_ends: k == 2,
                            ..s.pattern
                        };
                        nice_assert_no_alloc::assert_no_alloc(|| {
                            // Every MIDI key, then release half and play on.
                            for key in 0..128 {
                                arp.key_on(note(key));
                            }
                            for t in 0..4 {
                                arp.tick(Some(t as f64 * 0.25), 0.1, &s, &mut |_| events += 1);
                            }
                            for key in (0..128).step_by(2) {
                                arp.key_off(Some(key), None, s.latch);
                            }
                            s.triggered = true;
                            arp.trigger(note(0), &s, &mut |_| events += 1);
                            arp.release(Some(0), Some(0), &mut |_| events += 1);
                            s.triggered = false;
                            for t in 4..8 {
                                arp.tick(Some(t as f64 * 0.25), 0.1, &s, &mut |_| events += 1);
                            }
                            arp.key_off(None, None, false);
                            arp.reset();
                        });
                    }
                }
            }
        }
        assert!(events > 0);
    }
}
