//! Arpeggio patterns as walks over a virtual index space `0..m`, where `m` is the number of held
//! notes times the octave range. The engine maps index `i` to `pool[i % n] + 12 * (i / n)`, so a
//! pattern only depends on `m`, never on which notes are held.

// The derive is the only link to the plugin framework; `fill` itself is plain Rust.
use nice_plug::prelude::Enum;

/// Omnisphere 3's note patterns, in its menu order. Up/Down+ and Down/Up+ are Up/Down and Down/Up
/// with `repeat_ends` on.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[id = "chord"]
    Chord,
    #[id = "up"]
    Up,
    #[id = "down"]
    Down,
    #[id = "up-down"]
    #[name = "Up/Down"]
    UpDown,
    #[id = "down-up"]
    #[name = "Down/Up"]
    DownUp,
    #[id = "random"]
    Random,
    #[id = "as-played"]
    #[name = "As Played"]
    AsPlayed,
    #[id = "repeat-x2"]
    #[name = "Repeat X2"]
    RepeatX2,
    #[id = "repeat-x4"]
    #[name = "Repeat X4"]
    RepeatX4,
    #[id = "join"]
    Join,
    #[id = "spread"]
    Spread,
    #[id = "join-spread"]
    #[name = "Join/Spread"]
    JoinSpread,
    #[id = "spread-join"]
    #[name = "Spread/Join"]
    SpreadJoin,
    #[id = "stairs-up"]
    #[name = "Stairs Up"]
    StairsUp,
    #[id = "stairs-down"]
    #[name = "Stairs Down"]
    StairsDown,
    #[id = "stairs-up-down"]
    #[name = "Stairs Up/Down"]
    StairsUpDown,
    #[id = "stairs-down-up"]
    #[name = "Stairs Down/Up"]
    StairsDownUp,
}

/// Upper bound on the cycle length of any mode over `m` indices.
pub const fn max_len(m: usize) -> usize {
    4 * m
}

/// Replaces `out` with one cycle of `mode` over `0..m`. Doesn't allocate as long as
/// `out.capacity() >= max_len(m)`.
///
/// Chord, Random and As Played are plain Up here. The engine gives them their meaning: Chord walks
/// octaves and plays the whole pool at each step, Random reshuffles every cycle with [`shuffle`],
/// and As Played orders the pool by key press instead of pitch.
///
/// Stairs walks go +2, -1 from the bottom (`0, 2, 1, 3, 2, 4, ...`) and the cycle ends at the
/// first step that would leave `0..m`. A stair needs at least three indices to take its +2 step,
/// so with fewer the Stairs modes play their plain counterparts.
pub fn fill(out: &mut Vec<usize>, mode: Mode, m: usize, repeat_ends: bool) {
    out.clear();
    if m == 0 {
        return;
    }

    let mode = match mode {
        Mode::StairsUp if m < 3 => Mode::Up,
        Mode::StairsDown if m < 3 => Mode::Down,
        Mode::StairsUpDown if m < 3 => Mode::UpDown,
        Mode::StairsDownUp if m < 3 => Mode::DownUp,
        mode => mode,
    };
    match mode {
        Mode::Chord | Mode::Up | Mode::Random | Mode::AsPlayed => up(out, m),
        Mode::Down => down(out, m),
        Mode::UpDown => both(out, m, repeat_ends, up, down),
        Mode::DownUp => both(out, m, repeat_ends, down, up),
        Mode::RepeatX2 => out.extend((0..m).flat_map(|i| std::iter::repeat_n(i, 2))),
        Mode::RepeatX4 => out.extend((0..m).flat_map(|i| std::iter::repeat_n(i, 4))),
        Mode::Join => join(out, m),
        Mode::Spread => spread(out, m),
        Mode::JoinSpread => both(out, m, repeat_ends, join, spread),
        Mode::SpreadJoin => both(out, m, repeat_ends, spread, join),
        Mode::StairsUp => stairs_up(out, m),
        Mode::StairsDown => stairs_down(out, m),
        Mode::StairsUpDown => both(out, m, repeat_ends, stairs_up, stairs_down),
        Mode::StairsDownUp => both(out, m, repeat_ends, stairs_down, stairs_up),
    }
}

/// Shuffles `indices` in place with the xorshift64 generator in `state`, which must not be zero.
/// Swaps the first two if the shuffle would start on `previous`, so a new cycle doesn't repeat the
/// note that ended the last one.
pub fn shuffle(indices: &mut [usize], state: &mut u64, previous: usize) {
    for i in (1..indices.len()).rev() {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        indices.swap(i, (*state % (i as u64 + 1)) as usize);
    }
    if indices.len() > 1 && indices[0] == previous {
        indices.swap(0, 1);
    }
}

type Walk = fn(&mut Vec<usize>, usize);

/// Plays `first`, then `second`. Where the two halves meet, and where the cycle loops, a note that
/// would play twice in a row plays once unless `repeat_ends` is on.
fn both(out: &mut Vec<usize>, m: usize, repeat_ends: bool, first: Walk, second: Walk) {
    first(out, m);
    let seam = out.len();
    second(out, m);
    if !repeat_ends {
        if out[seam] == out[seam - 1] {
            out.remove(seam);
        }
        if out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
    }
}

fn up(out: &mut Vec<usize>, m: usize) {
    out.extend(0..m);
}

fn down(out: &mut Vec<usize>, m: usize) {
    out.extend((0..m).rev());
}

/// Outside in: lowest, highest, second lowest, second highest, and so on.
fn join(out: &mut Vec<usize>, m: usize) {
    for j in 0..m / 2 {
        out.extend([j, m - 1 - j]);
    }
    if m % 2 == 1 {
        out.push(m / 2);
    }
}

/// Inside out: Join's pairs in reverse order, each still low then high. An odd middle goes first.
fn spread(out: &mut Vec<usize>, m: usize) {
    if m % 2 == 1 {
        out.push(m / 2);
    }
    for j in (0..m / 2).rev() {
        out.extend([j, m - 1 - j]);
    }
}

/// `a[2j] = j, a[2j + 1] = j + 2`, stopping before `j + 2` reaches `m`. Needs `m >= 3`.
fn stairs_up(out: &mut Vec<usize>, m: usize) {
    for j in 0..m - 2 {
        out.extend([j, j + 2]);
    }
    out.push(m - 2);
}

fn stairs_down(out: &mut Vec<usize>, m: usize) {
    let start = out.len();
    stairs_up(out, m);
    mirror(&mut out[start..], m);
}

/// Reflects indices top to bottom, turning an upward walk into its downward twin.
fn mirror(indices: &mut [usize], m: usize) {
    for i in indices {
        *i = m - 1 - *i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [Mode; 17] = [
        Mode::Chord,
        Mode::Up,
        Mode::Down,
        Mode::UpDown,
        Mode::DownUp,
        Mode::Random,
        Mode::AsPlayed,
        Mode::RepeatX2,
        Mode::RepeatX4,
        Mode::Join,
        Mode::Spread,
        Mode::JoinSpread,
        Mode::SpreadJoin,
        Mode::StairsUp,
        Mode::StairsDown,
        Mode::StairsUpDown,
        Mode::StairsDownUp,
    ];

    fn pattern(mode: Mode, n: usize, octaves: usize, repeat_ends: bool) -> Vec<usize> {
        let mut out = Vec::new();
        fill(&mut out, mode, n * octaves, repeat_ends);
        out
    }

    #[test]
    fn known_cycles() {
        use Mode::*;
        let cases: &[(Mode, usize, bool, &[usize])] = &[
            (Up, 4, false, &[0, 1, 2, 3]),
            (Down, 4, false, &[3, 2, 1, 0]),
            (UpDown, 4, false, &[0, 1, 2, 3, 2, 1]),
            (UpDown, 4, true, &[0, 1, 2, 3, 3, 2, 1, 0]),
            (DownUp, 4, false, &[3, 2, 1, 0, 1, 2]),
            (DownUp, 4, true, &[3, 2, 1, 0, 0, 1, 2, 3]),
            (RepeatX2, 3, false, &[0, 0, 1, 1, 2, 2]),
            (RepeatX4, 2, false, &[0, 0, 0, 0, 1, 1, 1, 1]),
            // The manual's six-note examples: 1-6-2-5-3-4 and 3-4-2-5-1-6.
            (Join, 6, false, &[0, 5, 1, 4, 2, 3]),
            (Spread, 6, false, &[2, 3, 1, 4, 0, 5]),
            (Join, 5, false, &[0, 4, 1, 3, 2]),
            (Spread, 5, false, &[2, 1, 3, 0, 4]),
            (JoinSpread, 6, false, &[0, 5, 1, 4, 2, 3, 2, 3, 1, 4, 0, 5]),
            (SpreadJoin, 6, false, &[2, 3, 1, 4, 0, 5, 0, 5, 1, 4, 2, 3]),
            // An odd middle would play twice where Join and Spread meet.
            (JoinSpread, 5, false, &[0, 4, 1, 3, 2, 1, 3, 0, 4]),
            (JoinSpread, 5, true, &[0, 4, 1, 3, 2, 2, 1, 3, 0, 4]),
            (SpreadJoin, 5, false, &[2, 1, 3, 0, 4, 0, 4, 1, 3]),
            (StairsUp, 3, false, &[0, 2, 1]),
            (StairsUp, 5, false, &[0, 2, 1, 3, 2, 4, 3]),
            (StairsDown, 5, false, &[4, 2, 3, 1, 2, 0, 1]),
            (StairsUpDown, 4, false, &[0, 2, 1, 3, 2, 3, 1, 2, 0, 1]),
            (StairsDownUp, 4, false, &[3, 1, 2, 0, 1, 0, 2, 1, 3, 2]),
            // Too few indices for a stair: plain walks.
            (StairsUp, 2, false, &[0, 1]),
            (StairsDown, 2, false, &[1, 0]),
            (StairsUpDown, 2, false, &[0, 1]),
            // A single note just repeats.
            (StairsUpDown, 1, false, &[0]),
            (UpDown, 1, false, &[0]),
        ];
        for &(mode, m, repeat_ends, expected) in cases {
            assert_eq!(pattern(mode, m, 1, repeat_ends), expected, "{mode:?} m={m}");
        }
    }

    #[test]
    fn stairs_follow_the_formula_until_the_ceiling() {
        for m in 3..=18 {
            let p = pattern(Mode::StairsUp, m, 1, false);
            assert_eq!(p.len(), 2 * m - 3, "m={m}");
            for (k, &i) in p.iter().enumerate() {
                let j = k / 2;
                assert_eq!(i, if k % 2 == 0 { j } else { j + 2 }, "m={m} k={k}");
            }
            // The next step would be `m`, out of range: that is where the cycle ends.
            assert_eq!(p.len() / 2 + 2, m);
        }
    }

    #[test]
    fn every_mode_covers_the_whole_range_for_n_1_to_6_and_octaves_1_to_3() {
        for n in 1..=6 {
            for octaves in 1..=3 {
                let m = n * octaves;
                for mode in MODES {
                    for repeat_ends in [false, true] {
                        let p = pattern(mode, n, octaves, repeat_ends);
                        let ctx = format!("{mode:?} n={n} octaves={octaves} repeat={repeat_ends}");
                        assert!(!p.is_empty() && p.len() <= max_len(m), "{ctx}");
                        assert!(p.iter().all(|&i| i < m), "{ctx}: {p:?}");
                        assert!((0..m).all(|i| p.contains(&i)), "{ctx}: skips a note {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn no_note_plays_twice_in_a_row_unless_asked_to() {
        for m in 2..=18 {
            for mode in MODES {
                if matches!(mode, Mode::RepeatX2 | Mode::RepeatX4) {
                    continue;
                }
                let p = pattern(mode, m, 1, false);
                // Includes the seam from the last step back to the first.
                for (k, &i) in p.iter().enumerate() {
                    assert_ne!(i, p[(k + 1) % p.len()], "{mode:?} m={m}: {p:?}");
                }
            }
        }
    }

    #[test]
    fn downward_modes_mirror_upward_ones() {
        let pairs = [
            (Mode::Up, Mode::Down),
            (Mode::UpDown, Mode::DownUp),
            (Mode::StairsUp, Mode::StairsDown),
            (Mode::StairsUpDown, Mode::StairsDownUp),
        ];
        for m in 1..=18 {
            for (up, down) in pairs {
                for repeat_ends in [false, true] {
                    let mut mirrored = pattern(up, m, 1, repeat_ends);
                    mirror(&mut mirrored, m);
                    assert_eq!(mirrored, pattern(down, m, 1, repeat_ends), "{up:?} m={m}");
                }
            }
        }
    }

    #[test]
    fn shuffle_permutes_without_repeating_across_cycles() {
        let mut state = 0x9e37_79b9_7f4a_7c15;
        for m in 1..=18 {
            let mut p = pattern(Mode::Random, m, 1, false);
            let mut orders = std::collections::HashSet::new();
            for _ in 0..50 {
                let previous = *p.last().unwrap();
                shuffle(&mut p, &mut state, previous);
                let mut sorted = p.clone();
                sorted.sort();
                assert_eq!(sorted, (0..m).collect::<Vec<_>>(), "m={m}");
                if m > 1 {
                    assert_ne!(p[0], previous, "m={m}");
                }
                orders.insert(p.clone());
            }
            if m >= 3 {
                assert!(orders.len() > 1, "m={m}: the order never changes");
            }
        }
    }

    #[test]
    fn fill_stays_within_capacity() {
        let m = 128 * 4;
        let mut out = Vec::with_capacity(max_len(m));
        let capacity = out.capacity();
        for mode in MODES {
            for repeat_ends in [false, true] {
                fill(&mut out, mode, m, repeat_ends);
                assert_eq!(out.capacity(), capacity, "{mode:?} reallocated");
            }
        }
    }
}
