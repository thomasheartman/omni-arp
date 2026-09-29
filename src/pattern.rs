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
    /// Every note once per cycle, in a new random order each cycle. Start and Edge don't apply.
    #[id = "shuffle"]
    Shuffle,
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

/// A second line alongside the lead walk.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pair {
    #[id = "off"]
    Off,
    /// A second walker does the same shape the opposite way, alternating with the lead. Each
    /// keeps to its half of the range.
    #[id = "mirror"]
    Mirror,
    /// The lowest note alternates with a walk over the others.
    #[id = "low"]
    Low,
    /// The highest note alternates with a walk over the others.
    #[id = "high"]
    High,
    /// Each lead step again, an octave down. The engine plays these; the pattern is the lead's.
    #[id = "echo-below"]
    #[name = "Echo Below"]
    EchoBelow,
    /// Each lead step again, an octave up.
    #[id = "echo-above"]
    #[name = "Echo Above"]
    EchoAbove,
}

/// Which line plays first when there's a pair.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum First {
    #[id = "lead"]
    Lead,
    #[id = "pair"]
    Pair,
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
    pub first: First,
    /// Keep the step that a Reverse turnaround would play twice in a row.
    pub repeat_ends: bool,
    pub octave_behavior: OctaveBehavior,
}

/// Upper bound on the cycle length over `m` indices: two passes of at most `3m` notes each, and
/// a pedal note between every one of them.
pub const fn max_len(m: usize) -> usize {
    12 * m + 12
}

/// Replaces `out` with one cycle of `spec` over `n` notes in `octaves` octaves. `seed` drives
/// Shuffle; the same seed gives the same order. Doesn't allocate as long as
/// `out.capacity() >= max_len(n * octaves)`.
pub fn fill(out: &mut Vec<usize>, spec: &Spec, n: usize, octaves: usize, seed: u64) {
    out.clear();
    if n == 0 {
        return;
    }
    let rng = &mut seed.max(1);
    let down = spec.direction == Direction::Down;
    match spec.octave_behavior {
        OctaveBehavior::Thin => walk(out, spec, n * octaves, rng),
        OctaveBehavior::OneByOne => {
            walk(out, spec, n, rng);
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
            walk(out, spec, n, rng);
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

fn walk(out: &mut Vec<usize>, spec: &Spec, m: usize, rng: &mut u64) {
    let down = spec.direction == Direction::Down;
    let pair_first = spec.first == First::Pair;
    let pedal = match spec.pair {
        Pair::Low => 0,
        Pair::High => m - 1,
        Pair::Off | Pair::Mirror | Pair::EchoBelow | Pair::EchoAbove => {
            let paired = spec.pair == Pair::Mirror;
            if spec.shape != Shape::Shuffle {
                up(out, spec, paired, m);
            } else if paired {
                shuffled_halves(out, m, pair_first, rng);
            } else {
                out.extend(0..m);
                shuffle(out, rng);
            }
            if down {
                mirror(out, m);
            }
            return;
        }
    };

    // The walker covers every note but the pedal, and the two alternate.
    let walker = m - 1;
    if walker > 0 {
        if spec.shape == Shape::Shuffle {
            out.extend(0..walker);
            shuffle(out, rng);
        } else {
            up(out, spec, false, walker);
        }
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
        let step = out[k];
        let (a, b) = if pair_first {
            (pedal, step)
        } else {
            (step, pedal)
        };
        out[2 * k] = a;
        out[2 * k + 1] = b;
    }
    if out.is_empty() {
        out.push(pedal);
    }
}

/// One cycle heading up.
fn up(out: &mut Vec<usize>, spec: &Spec, paired: bool, m: usize) {
    if paired {
        let pair_first = spec.first == First::Pair;
        let first = pair_pass(out, spec.shape, spec.start, pair_first, m);
        // The walkers can't pass each other, so Wrap has nothing to wrap to.
        if spec.edge == Edge::Reverse {
            let seam = out.len();
            let other = match spec.start {
                Start::Outside => Start::Middle,
                Start::Middle => Start::Outside,
            };
            let second = pair_pass(out, spec.shape, other, pair_first, m);
            mirror(&mut out[seam..], m);
            drop_turnarounds(out, seam, first, second, spec.repeat_ends);
        }
        return;
    }

    let chunk = spec.shape.chunk(m);
    let from = match spec.start {
        Start::Outside => 0,
        Start::Middle => m / 2,
    };
    match spec.edge {
        Edge::Restart => out.extend(walk_up(chunk, from, m)),
        Edge::Wrap => {
            for base in from..from + m {
                out.extend(chunk.iter().map(|k| (base + k) % m));
            }
        }
        Edge::Reverse => {
            out.extend(walk_up(chunk, 0, m));
            let seam = out.len();
            out.extend(walk_up(chunk, 0, m));
            mirror(&mut out[seam..], m);
            drop_turnarounds(out, seam, (1, 1), (1, 1), spec.repeat_ends);
            // Same loop, entered at the middle: where its chunk starts, or where the note first
            // comes up if that chunk doesn't fit.
            let entry = if from + chunk[chunk.len() - 1] < m {
                from * chunk.len()
            } else {
                out.iter().position(|&i| i == from).unwrap_or(0)
            };
            out.rotate_left(entry);
        }
    }
}

/// Walks up from `from` a chunk at a time and stops after the last chunk that fits in `0..m`, so
/// the walk ends on the top note and a Reverse turns there. When whole chunks would play nothing
/// or skip a note (Stairs over three notes: `0 2`), it ends with a partial chunk instead: `0 2 1`.
fn walk_up(chunk: &[usize], from: usize, m: usize) -> impl Iterator<Item = usize> + '_ {
    let reach = chunk[chunk.len() - 1];
    let whole = m.saturating_sub(from).saturating_sub(reach);
    let gaps = reach + 1 > chunk.len();
    let bases = if whole == 0 || (gaps && whole == 1) {
        m
    } else {
        from + whole
    };
    (from..bases)
        .flat_map(move |base| chunk.iter().map(move |k| base + k))
        .take_while(move |&i| i < m)
}

/// Two walkers, each in its own half of the range (sharing the middle note when `m` is odd),
/// alternating notes. The leader walks up and the follower mirrors it: from the outside they walk
/// in toward the middle, from the middle out toward the edges. A note that would play twice in a
/// row plays once. Returns the number of notes in the first and last rounds.
fn pair_pass(
    out: &mut Vec<usize>,
    shape: Shape,
    start: Start,
    pair_first: bool,
    m: usize,
) -> (usize, usize) {
    let half = m.div_ceil(2);
    let (mut first, mut last) = (0, 0);
    let mut previous = None;
    for i in walk_up(shape.chunk(half), 0, half) {
        let (lead, follow) = match start {
            Start::Outside => (i, m - 1 - i),
            Start::Middle => (m - half + i, half - 1 - i),
        };
        let round = out.len();
        let order = if pair_first {
            [follow, lead]
        } else {
            [lead, follow]
        };
        for note in order {
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

/// Shuffle with Pair Mirror: the lower half (with the middle note, if any) and the upper half,
/// each in its own random order, alternating.
fn shuffled_halves(out: &mut Vec<usize>, m: usize, pair_first: bool, rng: &mut u64) {
    let lower = m.div_ceil(2);
    // The two orders go in scratch space past the result.
    out.resize(2 * m, 0);
    let (result, scratch) = out.split_at_mut(m);
    for (k, i) in scratch.iter_mut().enumerate() {
        *i = k;
    }
    let (low, high) = scratch.split_at_mut(lower);
    shuffle(low, rng);
    shuffle(high, rng);
    let (a, b) = if pair_first {
        (&*high, &*low)
    } else {
        (&*low, &*high)
    };
    let mut k = 0;
    for t in 0..lower {
        for part in [a, b] {
            if let Some(&i) = part.get(t) {
                result[k] = i;
                k += 1;
            }
        }
    }
    out.truncate(m);
}

/// Fisher-Yates with a xorshift64 generator; `rng` must not be zero.
fn shuffle(indices: &mut [usize], rng: &mut u64) {
    for i in (1..indices.len()).rev() {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        indices.swap(i, (*rng % (i as u64 + 1)) as usize);
    }
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

    const SHAPES: [Shape; 4] = [
        Shape::Straight,
        Shape::Stairs,
        Shape::GroupsOfThree,
        Shape::Shuffle,
    ];
    const EDGES: [Edge; 3] = [Edge::Restart, Edge::Reverse, Edge::Wrap];
    const PAIRS: [Pair; 6] = [
        Pair::Off,
        Pair::Mirror,
        Pair::Low,
        Pair::High,
        Pair::EchoBelow,
        Pair::EchoAbove,
    ];
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
            first: First::Lead,
            repeat_ends: false,
            octave_behavior: OctaveBehavior::Thin,
        }
    }

    fn pair_first(spec: Spec) -> Spec {
        Spec {
            first: First::Pair,
            ..spec
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
                                        [false, true].into_iter().flat_map(move |repeat_ends| {
                                            [First::Lead, First::Pair].map(|first| Spec {
                                                first,
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
        })
    }

    fn pattern(spec: &Spec, n: usize, octaves: usize) -> Vec<usize> {
        let mut out = Vec::new();
        fill(&mut out, spec, n, octaves, 42);
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
                &[0, 2, 1, 3, 2, 4],
            ),
            (
                spec(Stairs, Down, Outside, Restart, Off),
                5,
                &[4, 2, 3, 1, 2, 0],
            ),
            (
                spec(Stairs, Up, Outside, Reverse, Off),
                4,
                &[0, 2, 1, 3, 1, 2],
            ),
            (
                spec(Stairs, Down, Outside, Reverse, Off),
                4,
                &[3, 1, 2, 0, 2, 1],
            ),
            (
                spec(Stairs, Up, Outside, Wrap, Off),
                5,
                &[0, 2, 1, 3, 2, 4, 3, 0, 4, 1],
            ),
            (spec(Stairs, Up, Outside, Restart, Off), 2, &[0, 1]),
            // Whole pairs would skip the middle of three notes, so the last pair stays partial.
            (spec(Stairs, Up, Outside, Restart, Off), 3, &[0, 2, 1]),
            // C E G C': whole groups turn on the top note and come back down to the bottom.
            (
                spec(GroupsOfThree, Up, Outside, Reverse, Off),
                4,
                &[0, 1, 2, 1, 2, 3, 2, 1, 2, 1],
            ),
            (
                spec(GroupsOfThree, Up, Outside, Restart, Off),
                5,
                &[0, 1, 2, 1, 2, 3, 2, 3, 4],
            ),
            (
                spec(GroupsOfThree, Down, Outside, Restart, Off),
                5,
                &[4, 3, 2, 3, 2, 1, 2, 1, 0],
            ),
            (
                spec(GroupsOfThree, Up, Outside, Reverse, Off),
                3,
                &[0, 1, 2, 1],
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
                &[1, 0, 2, 0, 3, 0, 4, 0],
            ),
            (
                spec(Straight, Down, Outside, Restart, Low),
                5,
                &[4, 0, 3, 0, 2, 0, 1, 0],
            ),
            (
                spec(Straight, Up, Outside, Reverse, Low),
                5,
                &[1, 0, 2, 0, 3, 0, 4, 0, 3, 0, 2, 0],
            ),
            (
                spec(Straight, Down, Outside, Reverse, Low),
                5,
                &[4, 0, 3, 0, 2, 0, 1, 0, 2, 0, 3, 0],
            ),
            (
                spec(Straight, Up, Outside, Restart, High),
                5,
                &[0, 4, 1, 4, 2, 4, 3, 4],
            ),
            (
                spec(Straight, Down, Outside, Restart, High),
                5,
                &[3, 4, 2, 4, 1, 4, 0, 4],
            ),
            (spec(Straight, Up, Outside, Restart, Low), 2, &[1, 0]),
            // First: Pair puts the pedal or the mirrored walker first.
            (
                pair_first(spec(Straight, Up, Outside, Restart, Low)),
                5,
                &[0, 1, 0, 2, 0, 3, 0, 4],
            ),
            (
                pair_first(spec(Straight, Up, Outside, Restart, Mirror)),
                6,
                &[5, 0, 4, 1, 3, 2],
            ),
            (
                pair_first(spec(Straight, Up, Outside, Reverse, Mirror)),
                6,
                &[5, 0, 4, 1, 3, 2, 4, 1],
            ),
            // Echo is played by the engine; the pattern is the lead's.
            (
                spec(Stairs, Up, Outside, Restart, EchoBelow),
                4,
                &[0, 2, 1, 3],
            ),
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
        for m in 4..=18 {
            let p = pattern(&stairs, m, 1);
            // Pairs up to the last one that fits, which ends on the top note.
            assert_eq!(p.len(), 2 * m - 4, "m={m}");
            assert_eq!(p[p.len() - 1], m - 1, "m={m}");
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
                        && spec.pair != Pair::Mirror
                        && spec.shape != Shape::Shuffle;
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
    fn shuffle_draws_a_new_order_from_each_seed() {
        let shuffled = |pair| {
            spec(
                Shape::Shuffle,
                Direction::Up,
                Start::Outside,
                Edge::Restart,
                pair,
            )
        };
        let fill_with = |spec: &Spec, m: usize, seed: u64| {
            let mut out = Vec::new();
            fill(&mut out, spec, m, 1, seed);
            out
        };
        for m in 1..=12 {
            let mut orders = std::collections::HashSet::new();
            for seed in 1..=40 {
                let p = fill_with(&shuffled(Pair::Off), m, seed);
                let mut sorted = p.clone();
                sorted.sort();
                assert_eq!(sorted, (0..m).collect::<Vec<_>>(), "m={m}");
                orders.insert(p);

                // Mirror alternates the lower half (with an odd middle) and the upper half.
                let p = fill_with(&shuffled(Pair::Mirror), m, seed);
                let lower = m.div_ceil(2);
                for (k, &i) in p.iter().enumerate().take(2 * (m - lower)) {
                    assert_eq!(i < lower, k % 2 == 0, "m={m}: {p:?}");
                }

                // Low keeps its pedal note between the shuffled others.
                let p = fill_with(&shuffled(Pair::Low), m, seed);
                if m > 1 {
                    assert!(p.iter().skip(1).step_by(2).all(|&i| i == 0), "m={m}: {p:?}");
                }
            }
            if m >= 3 {
                assert!(orders.len() > 1, "m={m}: the order never changes");
            }
        }
        // The same seed gives the same order.
        assert_eq!(
            fill_with(&shuffled(Pair::Off), 8, 7),
            fill_with(&shuffled(Pair::Off), 8, 7)
        );
    }

    #[test]
    fn fill_stays_within_capacity() {
        let (n, octaves) = (128, 7);
        let mut out = Vec::with_capacity(max_len(n * octaves));
        let capacity = out.capacity();
        for spec in every_spec() {
            fill(&mut out, &spec, n, octaves, 42);
            assert_eq!(out.capacity(), capacity, "{spec:?} reallocated");
        }
    }
}
