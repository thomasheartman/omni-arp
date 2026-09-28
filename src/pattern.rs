//! Arpeggio patterns as walks over a virtual index space `0..m`, where `m` is the number of held
//! notes times the number of octaves. The engine maps index `i` to `pool[i % n]`, shifted by
//! `i / n` octaves, so a pattern depends only on the counts, never on which notes are held.
//!
//! Every pattern is generated going up and mirrored (`i -> m - 1 - i`) for Direction: Down.

// The derives are the only link to the plugin framework; `fill` itself is plain Rust.
use nice_plug::prelude::Enum;

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    #[id = "straight"]
    Straight,
    #[id = "stairs"]
    Stairs,
    #[id = "groups-of-three"]
    #[name = "Groups of Three"]
    GroupsOfThree,
    #[id = "repeat-x2"]
    #[name = "Repeat x2"]
    RepeatX2,
    #[id = "repeat-x4"]
    #[name = "Repeat x4"]
    RepeatX4,
}

impl Shape {
    /// Notes played from each position before a walk over `m` notes moves on by one: Stairs is
    /// `+2 -1`, Groups of Three `+1 +1 -1`. Both need three notes, and walk straight over fewer.
    fn chunk(self, m: usize) -> &'static [usize] {
        match self {
            Shape::Stairs if m >= 3 => &[0, 2],
            Shape::GroupsOfThree if m >= 3 => &[0, 1, 2],
            _ => &[0],
        }
    }

    /// The Repeat shapes walk straight; the engine plays each of their steps this many times.
    pub fn repeats(self) -> usize {
        match self {
            Shape::RepeatX2 => 2,
            Shape::RepeatX4 => 4,
            _ => 1,
        }
    }
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    #[id = "up"]
    Up,
    #[id = "down"]
    Down,
}

/// Where the walk starts. Outside is the end opposite the direction; Middle is the middle note
/// on the side the walk is heading.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    #[id = "outside"]
    Outside,
    #[id = "middle"]
    Middle,
}

/// What the walk does when its next note would leave the range.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// Start over from the start.
    #[id = "restart"]
    Restart,
    /// Turn around and walk back from the end just reached.
    #[id = "reverse"]
    Reverse,
    /// Continue from the other end of the range.
    #[id = "wrap"]
    Wrap,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pair {
    #[id = "off"]
    Off,
    /// A second walker does the same shape the opposite way, alternating with the first. Each
    /// keeps to its half of the range.
    #[id = "mirror"]
    Mirror,
    /// The lowest note plays between every step of a walk over the others.
    #[id = "low"]
    Low,
    /// The highest note plays between every step of a walk over the others.
    #[id = "high"]
    High,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OctaveBehavior {
    /// One walk over the notes of every octave.
    #[id = "thin"]
    Thin,
    /// The whole pattern in one octave, then in the next.
    #[id = "1-by-1"]
    #[name = "1 by 1"]
    OneByOne,
    /// Each step's note in every octave before the next step.
    #[id = "alt"]
    Alt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    pub shape: Shape,
    pub direction: Direction,
    pub start: Start,
    pub edge: Edge,
    pub pair: Pair,
    /// Keep the step that a Reverse turnaround would play twice in a row.
    pub repeat_ends: bool,
    pub octave_behavior: OctaveBehavior,
}

/// Upper bound on the cycle length over `m` indices: two passes of at most `3m` notes each, and
/// a pedal note between every one of them.
pub const fn max_len(m: usize) -> usize {
    12 * m + 12
}

/// Replaces `out` with one cycle of `spec` over `n` notes in `octaves` octaves. Doesn't allocate
/// as long as `out.capacity() >= max_len(n * octaves)`.
pub fn fill(out: &mut Vec<usize>, spec: &Spec, n: usize, octaves: usize) {
    out.clear();
    if n == 0 {
        return;
    }
    let down = spec.direction == Direction::Down;
    match spec.octave_behavior {
        OctaveBehavior::Thin => walk(out, spec, n * octaves),
        OctaveBehavior::OneByOne => {
            walk(out, spec, n);
            let len = out.len();
            out.resize(len * octaves, 0);
            // Back to front, so the first copy is still intact while the others read it.
            for t in (0..octaves).rev() {
                let shift = n * if down { octaves - 1 - t } else { t };
                for k in 0..len {
                    out[t * len + k] = out[k] + shift;
                }
            }
        }
        OctaveBehavior::Alt => {
            walk(out, spec, n);
            let len = out.len();
            out.resize(len * octaves, 0);
            for k in (0..len).rev() {
                let i = out[k];
                for t in 0..octaves {
                    out[k * octaves + t] = i + n * if down { octaves - 1 - t } else { t };
                }
            }
        }
    }
}

fn walk(out: &mut Vec<usize>, spec: &Spec, m: usize) {
    let down = spec.direction == Direction::Down;
    let pedal = match spec.pair {
        Pair::Low => 0,
        Pair::High => m - 1,
        Pair::Off | Pair::Mirror => {
            let paired = spec.pair == Pair::Mirror;
            up(
                out,
                spec.shape,
                spec.start,
                spec.edge,
                paired,
                spec.repeat_ends,
                m,
            );
            if down {
                mirror(out, m);
            }
            return;
        }
    };

    // The walker covers every note but the pedal, and the pedal goes before each of its steps.
    let walker = m - 1;
    if walker > 0 {
        up(
            out,
            spec.shape,
            spec.start,
            spec.edge,
            false,
            spec.repeat_ends,
            walker,
        );
        if down {
            mirror(out, walker);
        }
        if spec.pair == Pair::Low {
            out.iter_mut().for_each(|i| *i += 1);
        }
    }
    let len = out.len();
    out.resize(2 * len, 0);
    for k in (0..len).rev() {
        out[2 * k + 1] = out[k];
        out[2 * k] = pedal;
    }
    if out.is_empty() {
        out.push(pedal);
    }
}

/// One cycle heading up.
fn up(
    out: &mut Vec<usize>,
    shape: Shape,
    start: Start,
    edge: Edge,
    paired: bool,
    repeat_ends: bool,
    m: usize,
) {
    if paired {
        let first = pair_pass(out, shape, start, m);
        // The walkers can't pass each other, so Wrap has nothing to wrap to.
        if edge == Edge::Reverse {
            let seam = out.len();
            let other = match start {
                Start::Outside => Start::Middle,
                Start::Middle => Start::Outside,
            };
            let second = pair_pass(out, shape, other, m);
            mirror(&mut out[seam..], m);
            drop_turnarounds(out, seam, first, second, repeat_ends);
        }
        return;
    }

    let chunk = shape.chunk(m);
    let from = match start {
        Start::Outside => 0,
        Start::Middle => m / 2,
    };
    match edge {
        Edge::Restart => pass(out, chunk, from, m),
        Edge::Wrap => {
            for base in from..from + m {
                out.extend(chunk.iter().map(|k| (base + k) % m));
            }
        }
        Edge::Reverse => {
            pass(out, chunk, 0, m);
            let seam = out.len();
            pass(out, chunk, 0, m);
            mirror(&mut out[seam..], m);
            drop_turnarounds(out, seam, (1, 1), (1, 1), repeat_ends);
            // Same loop, entered at the middle. Every chunk before it is whole.
            out.rotate_left(from * chunk.len());
        }
    }
}

/// Walks up from `from` a chunk at a time, stopping before the first note outside `0..m`.
fn pass(out: &mut Vec<usize>, chunk: &[usize], from: usize, m: usize) {
    out.extend(walk_up(chunk, from, m));
}

fn walk_up(chunk: &[usize], from: usize, m: usize) -> impl Iterator<Item = usize> + '_ {
    (from..m)
        .flat_map(move |base| chunk.iter().map(move |k| base + k))
        .take_while(move |&i| i < m)
}

/// Two walkers, each in its own half of the range (sharing the middle note when `m` is odd),
/// alternating notes. The leader walks up and the follower mirrors it: from the outside they walk
/// in toward the middle, from the middle out toward the edges. A note that would play twice in a
/// row plays once. Returns the number of notes in the first and last rounds.
fn pair_pass(out: &mut Vec<usize>, shape: Shape, start: Start, m: usize) -> (usize, usize) {
    let half = m.div_ceil(2);
    let (mut first, mut last) = (0, 0);
    let mut previous = None;
    for i in walk_up(shape.chunk(half), 0, half) {
        let (lead, follow) = match start {
            Start::Outside => (i, m - 1 - i),
            Start::Middle => (m - half + i, half - 1 - i),
        };
        let round = out.len();
        for note in [lead, follow] {
            if previous != Some(note) {
                out.push(note);
                previous = Some(note);
            }
        }
        if out.len() > round {
            last = out.len() - round;
            if first == 0 {
                first = last;
            }
        }
    }
    (first, last)
}

/// `out[seam..]` is the walk back after `out[..seam]`. Unless `repeat_ends`, a step that would
/// play twice in a row where they meet, or where the cycle loops, plays once. A step is a note,
/// or a round of both walkers; `first` and `last` are the sizes of each part's first and last.
fn drop_turnarounds(
    out: &mut Vec<usize>,
    seam: usize,
    first: (usize, usize),
    second: (usize, usize),
    repeat_ends: bool,
) {
    if repeat_ends {
        return;
    }
    if out[seam - first.1..seam] == out[seam..seam + second.0] {
        out.drain(seam..seam + second.0);
    }
    // If the walk back was a single step, it's gone and the loop seam is already fine.
    let len = out.len();
    if len > seam && out[len - second.1..] == out[..first.0] {
        out.truncate(len - second.1);
    }
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

    const SHAPES: [Shape; 5] = [
        Shape::Straight,
        Shape::Stairs,
        Shape::GroupsOfThree,
        Shape::RepeatX2,
        Shape::RepeatX4,
    ];
    const EDGES: [Edge; 3] = [Edge::Restart, Edge::Reverse, Edge::Wrap];
    const PAIRS: [Pair; 4] = [Pair::Off, Pair::Mirror, Pair::Low, Pair::High];
    const BEHAVIORS: [OctaveBehavior; 3] = [
        OctaveBehavior::Thin,
        OctaveBehavior::OneByOne,
        OctaveBehavior::Alt,
    ];

    fn spec(shape: Shape, direction: Direction, start: Start, edge: Edge, pair: Pair) -> Spec {
        Spec {
            shape,
            direction,
            start,
            edge,
            pair,
            repeat_ends: false,
            octave_behavior: OctaveBehavior::Thin,
        }
    }

    fn every_spec() -> impl Iterator<Item = Spec> {
        SHAPES.into_iter().flat_map(|shape| {
            [Direction::Up, Direction::Down]
                .into_iter()
                .flat_map(move |direction| {
                    [Start::Outside, Start::Middle]
                        .into_iter()
                        .flat_map(move |start| {
                            EDGES.into_iter().flat_map(move |edge| {
                                PAIRS.into_iter().flat_map(move |pair| {
                                    BEHAVIORS.into_iter().flat_map(move |octave_behavior| {
                                        [false, true].map(|repeat_ends| Spec {
                                            repeat_ends,
                                            octave_behavior,
                                            ..spec(shape, direction, start, edge, pair)
                                        })
                                    })
                                })
                            })
                        })
                })
        })
    }

    fn pattern(spec: &Spec, n: usize, octaves: usize) -> Vec<usize> {
        let mut out = Vec::new();
        fill(&mut out, spec, n, octaves);
        out
    }

    #[test]
    fn known_cycles() {
        use Direction::*;
        use Edge::*;
        use Pair::*;
        use Shape::*;
        use Start::*;
        let cases: &[(Spec, usize, &[usize])] = &[
            (spec(Straight, Up, Outside, Restart, Off), 4, &[0, 1, 2, 3]),
            (
                spec(Straight, Down, Outside, Restart, Off),
                4,
                &[3, 2, 1, 0],
            ),
            (
                spec(Straight, Up, Outside, Reverse, Off),
                4,
                &[0, 1, 2, 3, 2, 1],
            ),
            (
                spec(Straight, Down, Outside, Reverse, Off),
                4,
                &[3, 2, 1, 0, 1, 2],
            ),
            (spec(Straight, Up, Outside, Wrap, Off), 4, &[0, 1, 2, 3]),
            // Middle: the upper middle note going up, the lower going down.
            (spec(Straight, Up, Middle, Restart, Off), 6, &[3, 4, 5]),
            (spec(Straight, Up, Middle, Restart, Off), 5, &[2, 3, 4]),
            (spec(Straight, Down, Middle, Restart, Off), 6, &[2, 1, 0]),
            (
                spec(Straight, Up, Middle, Wrap, Off),
                6,
                &[3, 4, 5, 0, 1, 2],
            ),
            (
                spec(Straight, Up, Middle, Reverse, Off),
                6,
                &[3, 4, 5, 4, 3, 2, 1, 0, 1, 2],
            ),
            (
                spec(Stairs, Up, Outside, Restart, Off),
                5,
                &[0, 2, 1, 3, 2, 4, 3],
            ),
            (
                spec(Stairs, Down, Outside, Restart, Off),
                5,
                &[4, 2, 3, 1, 2, 0, 1],
            ),
            (
                spec(Stairs, Up, Outside, Reverse, Off),
                4,
                &[0, 2, 1, 3, 2, 3, 1, 2, 0, 1],
            ),
            (
                spec(Stairs, Down, Outside, Reverse, Off),
                4,
                &[3, 1, 2, 0, 1, 0, 2, 1, 3, 2],
            ),
            (
                spec(Stairs, Up, Outside, Wrap, Off),
                5,
                &[0, 2, 1, 3, 2, 4, 3, 0, 4, 1],
            ),
            (spec(Stairs, Up, Outside, Restart, Off), 2, &[0, 1]),
            (
                spec(GroupsOfThree, Up, Outside, Restart, Off),
                5,
                &[0, 1, 2, 1, 2, 3, 2, 3, 4, 3, 4],
            ),
            (
                spec(GroupsOfThree, Down, Outside, Restart, Off),
                5,
                &[4, 3, 2, 3, 2, 1, 2, 1, 0, 1, 0],
            ),
            (
                spec(GroupsOfThree, Up, Outside, Reverse, Off),
                3,
                &[0, 1, 2, 1, 2, 1, 0, 1],
            ),
            // Join, Spread and their combinations, as in the manual's six-note examples.
            (
                spec(Straight, Up, Outside, Restart, Mirror),
                6,
                &[0, 5, 1, 4, 2, 3],
            ),
            (
                spec(Straight, Up, Outside, Restart, Mirror),
                5,
                &[0, 4, 1, 3, 2],
            ),
            (
                spec(Straight, Down, Middle, Restart, Mirror),
                6,
                &[2, 3, 1, 4, 0, 5],
            ),
            (
                spec(Straight, Down, Middle, Restart, Mirror),
                5,
                &[2, 1, 3, 0, 4],
            ),
            (
                spec(Straight, Up, Middle, Restart, Mirror),
                6,
                &[3, 2, 4, 1, 5, 0],
            ),
            (
                spec(Straight, Up, Outside, Reverse, Mirror),
                6,
                &[0, 5, 1, 4, 2, 3, 1, 4],
            ),
            (
                spec(Straight, Up, Outside, Reverse, Mirror),
                5,
                &[0, 4, 1, 3, 2, 1, 3],
            ),
            (
                spec(Straight, Down, Middle, Reverse, Mirror),
                6,
                &[2, 3, 1, 4, 0, 5, 1, 4],
            ),
            (
                spec(Straight, Down, Middle, Reverse, Mirror),
                5,
                &[2, 1, 3, 0, 4, 1, 3],
            ),
            (
                spec(Straight, Up, Outside, Restart, Low),
                5,
                &[0, 1, 0, 2, 0, 3, 0, 4],
            ),
            (
                spec(Straight, Down, Outside, Restart, Low),
                5,
                &[0, 4, 0, 3, 0, 2, 0, 1],
            ),
            (
                spec(Straight, Up, Outside, Reverse, Low),
                5,
                &[0, 1, 0, 2, 0, 3, 0, 4, 0, 3, 0, 2],
            ),
            (
                spec(Straight, Down, Outside, Reverse, Low),
                5,
                &[0, 4, 0, 3, 0, 2, 0, 1, 0, 2, 0, 3],
            ),
            (
                spec(Straight, Up, Outside, Restart, High),
                5,
                &[4, 0, 4, 1, 4, 2, 4, 3],
            ),
            (
                spec(Straight, Down, Outside, Restart, High),
                5,
                &[4, 3, 4, 2, 4, 1, 4, 0],
            ),
            (spec(Straight, Up, Outside, Restart, Low), 2, &[0, 1]),
            (spec(Straight, Up, Outside, Restart, Low), 1, &[0]),
            // A single note just repeats.
            (spec(Stairs, Up, Outside, Reverse, Mirror), 1, &[0]),
            (spec(GroupsOfThree, Down, Middle, Wrap, Off), 1, &[0]),
            // Mirrored stairs, each walker in its half: 0 2 1 below, 5 3 4 above.
            (
                spec(Stairs, Up, Outside, Restart, Mirror),
                6,
                &[0, 5, 2, 3, 1, 4],
            ),
            // The halves share the middle note, which plays once.
            (
                spec(Stairs, Up, Outside, Restart, Mirror),
                5,
                &[0, 4, 2, 1, 3],
            ),
        ];
        for (spec, m, expected) in cases {
            assert_eq!(pattern(spec, *m, 1), *expected, "{spec:?} m={m}");
        }
    }

    #[test]
    fn repeat_ends_keeps_the_turnarounds() {
        use Direction::*;
        use Edge::*;
        use Pair::*;
        use Shape::*;
        use Start::*;
        let cases: &[(Spec, usize, &[usize])] = &[
            (
                spec(Straight, Up, Outside, Reverse, Off),
                4,
                &[0, 1, 2, 3, 3, 2, 1, 0],
            ),
            (
                spec(Straight, Down, Outside, Reverse, Off),
                4,
                &[3, 2, 1, 0, 0, 1, 2, 3],
            ),
            (
                spec(Straight, Up, Outside, Reverse, Mirror),
                6,
                &[0, 5, 1, 4, 2, 3, 2, 3, 1, 4, 0, 5],
            ),
            (
                spec(Straight, Up, Outside, Reverse, Mirror),
                5,
                &[0, 4, 1, 3, 2, 2, 1, 3, 0, 4],
            ),
            (
                spec(Straight, Down, Middle, Reverse, Mirror),
                5,
                &[2, 1, 3, 0, 4, 0, 4, 1, 3, 2],
            ),
        ];
        for (spec, m, expected) in cases {
            let spec = Spec {
                repeat_ends: true,
                ..*spec
            };
            assert_eq!(pattern(&spec, *m, 1), *expected, "{spec:?} m={m}");
        }
    }

    #[test]
    fn octave_behaviors() {
        let up_down = spec(
            Shape::Straight,
            Direction::Up,
            Start::Outside,
            Edge::Reverse,
            Pair::Off,
        );
        let down = spec(
            Shape::Straight,
            Direction::Down,
            Start::Outside,
            Edge::Restart,
            Pair::Off,
        );
        let with = |spec: Spec, octave_behavior| Spec {
            octave_behavior,
            ..spec
        };
        // Three notes, two octaves: indices 3..6 are the octave above.
        assert_eq!(pattern(&up_down, 3, 2), [0, 1, 2, 3, 4, 5, 4, 3, 2, 1]);
        assert_eq!(
            pattern(&with(up_down, OctaveBehavior::OneByOne), 3, 2),
            [0, 1, 2, 1, 3, 4, 5, 4]
        );
        assert_eq!(
            pattern(&with(up_down, OctaveBehavior::Alt), 3, 2),
            [0, 3, 1, 4, 2, 5, 1, 4]
        );
        // Going down, the octaves go down too.
        assert_eq!(
            pattern(&with(down, OctaveBehavior::OneByOne), 3, 2),
            [5, 4, 3, 2, 1, 0]
        );
        assert_eq!(
            pattern(&with(down, OctaveBehavior::Alt), 3, 2),
            [5, 2, 4, 1, 3, 0]
        );
    }

    #[test]
    fn stairs_follow_the_formula_until_the_ceiling() {
        let stairs = spec(
            Shape::Stairs,
            Direction::Up,
            Start::Outside,
            Edge::Restart,
            Pair::Off,
        );
        for m in 3..=18 {
            let p = pattern(&stairs, m, 1);
            assert_eq!(p.len(), 2 * m - 3, "m={m}");
            for (k, &i) in p.iter().enumerate() {
                let j = k / 2;
                assert_eq!(i, if k % 2 == 0 { j } else { j + 2 }, "m={m} k={k}");
            }
        }
    }

    #[test]
    fn every_spec_stays_in_range_and_covers_it_for_n_1_to_6_and_octaves_1_to_3() {
        for spec in every_spec() {
            for n in 1..=6 {
                for octaves in 1..=3 {
                    let m = n * octaves;
                    let p = pattern(&spec, n, octaves);
                    let ctx = format!("{spec:?} n={n} octaves={octaves}: {p:?}");
                    assert!(!p.is_empty() && p.len() <= max_len(m), "{ctx}");
                    assert!(p.iter().all(|&i| i < m), "{ctx}");
                    // Restarting from the middle plays one half by design; Mirror covers both.
                    let half = spec.start == Start::Middle
                        && spec.edge == Edge::Restart
                        && spec.pair != Pair::Mirror;
                    if !half {
                        assert!((0..m).all(|i| p.contains(&i)), "{ctx}");
                    }
                }
            }
        }
    }

    #[test]
    fn no_note_plays_twice_in_a_row_unless_repeat_ends() {
        for spec in every_spec().filter(|s| !s.repeat_ends) {
            for n in 1..=6 {
                for octaves in 1..=3 {
                    let p = pattern(&spec, n, octaves);
                    // One note, or half of two: a single note repeating is the point.
                    if p.len() < 2 {
                        continue;
                    }
                    // Includes the seam from the last step back to the first.
                    for (k, &i) in p.iter().enumerate() {
                        let next = p[(k + 1) % p.len()];
                        assert_ne!(i, next, "{spec:?} n={n} octaves={octaves}: {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn down_mirrors_up() {
        for up in every_spec().filter(|s| {
            s.direction == Direction::Up
                && s.octave_behavior == OctaveBehavior::Thin
                && matches!(s.pair, Pair::Off | Pair::Mirror)
        }) {
            let down = Spec {
                direction: Direction::Down,
                ..up
            };
            for m in 1..=12 {
                let mut mirrored = pattern(&up, m, 1);
                mirror(&mut mirrored, m);
                assert_eq!(mirrored, pattern(&down, m, 1), "{up:?} m={m}");
            }
        }
    }

    #[test]
    fn fill_stays_within_capacity() {
        let (n, octaves) = (128, 7);
        let mut out = Vec::with_capacity(max_len(n * octaves));
        let capacity = out.capacity();
        for spec in every_spec() {
            fill(&mut out, &spec, n, octaves);
            assert_eq!(out.capacity(), capacity, "{spec:?} reallocated");
        }
    }
}
