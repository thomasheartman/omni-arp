//! Arpeggio patterns as walks over a virtual index space `0..m`, where `m` is the number of held
//! notes times the octave range. The engine maps index `i` to `pool[i % n] + 12 * (i / n)`, so a
//! pattern only depends on `m`, never on which notes are held.

// The derive is the only link to the plugin framework; `fill` itself is plain Rust.
use nice_plug::prelude::Enum;

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
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
/// Stairs walks go +2, -1 from the bottom (`0, 2, 1, 3, 2, 4, ...`) and the cycle ends at the
/// first step that would leave `0..m`. A stair needs at least three indices to take its +2 step,
/// so with fewer the Stairs modes play their plain counterparts. `repeat_ends` only affects
/// Up/Down and Down/Up: it plays the top and bottom notes twice at the turnarounds.
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
        Mode::Up => out.extend(0..m),
        Mode::Down => out.extend((0..m).rev()),
        Mode::UpDown => up_down(out, m, repeat_ends),
        Mode::DownUp => {
            up_down(out, m, repeat_ends);
            mirror(out, m);
        }
        Mode::StairsUp => stairs_up(out, m),
        Mode::StairsDown => {
            stairs_up(out, m);
            mirror(out, m);
        }
        Mode::StairsUpDown => stairs_up_down(out, m),
        Mode::StairsDownUp => {
            stairs_up_down(out, m);
            mirror(out, m);
        }
    }
}

fn up_down(out: &mut Vec<usize>, m: usize, repeat_ends: bool) {
    out.extend(0..m);
    if repeat_ends {
        out.extend((0..m).rev());
    } else {
        out.extend((1..m - 1).rev());
    }
}

/// `a[2j] = j, a[2j + 1] = j + 2`, stopping before `j + 2` reaches `m`. Needs `m >= 3`.
fn stairs_up(out: &mut Vec<usize>, m: usize) {
    for j in 0..m - 2 {
        out.extend([j, j + 2]);
    }
    out.push(m - 2);
}

/// Stairs Up ends on `m - 2` and Stairs Down starts on `m - 1` (and ends on 1 before looping to
/// 0), so neither seam repeats a note.
fn stairs_up_down(out: &mut Vec<usize>, m: usize) {
    stairs_up(out, m);
    let half = out.len();
    out.extend_from_within(..);
    mirror(&mut out[half..], m);
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

    const MODES: [Mode; 8] = [
        Mode::Up,
        Mode::Down,
        Mode::UpDown,
        Mode::DownUp,
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
    fn no_note_plays_twice_in_a_row_unless_repeat_ends() {
        for m in 2..=18 {
            for mode in MODES {
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
