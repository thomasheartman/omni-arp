use nice_plug::{
    midi::{Channel, Key, VoiceID},
    prelude::*,
};
use std::sync::Arc;

mod arp;
mod pattern;

use arp::{Arp, Note, Out, Settings, Velocity};
use pattern::{Direction, Edge, OctaveBehavior, Pair, Shape, Spec, Start};

pub struct StairsArp {
    params: Arc<ArpParams>,
    arp: Arp,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq)]
enum Rate {
    #[id = "1/4"]
    #[name = "1/4"]
    Quarter,
    #[id = "1/8"]
    #[name = "1/8"]
    Eighth,
    #[id = "1/8t"]
    #[name = "1/8T"]
    EighthTriplet,
    #[id = "1/16"]
    #[name = "1/16"]
    Sixteenth,
    #[id = "1/16t"]
    #[name = "1/16T"]
    SixteenthTriplet,
    #[id = "1/32"]
    #[name = "1/32"]
    ThirtySecond,
}

impl Rate {
    /// Step length in quarter notes.
    fn beats(self) -> f64 {
        match self {
            Rate::Quarter => 1.0,
            Rate::Eighth => 0.5,
            Rate::EighthTriplet => 1.0 / 3.0,
            Rate::Sixteenth => 0.25,
            Rate::SixteenthTriplet => 1.0 / 6.0,
            Rate::ThirtySecond => 0.125,
        }
    }
}

#[derive(Enum, Debug, Clone, Copy, PartialEq)]
enum Notes {
    #[id = "1"]
    #[name = "1"]
    One,
    #[id = "2"]
    #[name = "2"]
    Two,
    #[id = "3"]
    #[name = "3"]
    Three,
    #[id = "all"]
    All,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq)]
enum VelocityMode {
    #[id = "played"]
    #[name = "As Played"]
    AsPlayed,
    #[id = "fixed"]
    Fixed,
    #[id = "trigger"]
    #[name = "From Trigger"]
    FromTrigger,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq)]
enum Advance {
    #[id = "tempo"]
    Tempo,
    #[id = "trigger"]
    Trigger,
}

/// In the order of the remote control pages: pattern, notes, input.
#[derive(Params)]
struct ArpParams {
    #[id = "shape"]
    shape: EnumParam<Shape>,
    #[id = "direction"]
    direction: EnumParam<Direction>,
    #[id = "start"]
    start: EnumParam<Start>,
    #[id = "edge"]
    edge: EnumParam<Edge>,
    #[id = "pair"]
    pair: EnumParam<Pair>,
    #[id = "repeat-ends"]
    repeat_ends: BoolParam,
    #[id = "length"]
    length: IntParam,
    #[id = "rate"]
    rate: EnumParam<Rate>,

    #[id = "notes"]
    notes: EnumParam<Notes>,
    #[id = "chord-velocity"]
    chord_velocity: FloatParam,
    #[id = "note-length"]
    note_length: FloatParam,
    #[id = "octaves-down"]
    octaves_down: IntParam,
    #[id = "octaves-up"]
    octaves_up: IntParam,
    #[id = "octave-behavior"]
    octave_behavior: EnumParam<OctaveBehavior>,
    #[id = "velocity-mode"]
    velocity_mode: EnumParam<VelocityMode>,
    #[id = "velocity"]
    velocity: IntParam,

    #[id = "advance"]
    advance: EnumParam<Advance>,
    #[id = "trigger-channel"]
    trigger_channel: IntParam,
    #[id = "latch"]
    latch: BoolParam,
    #[id = "restart-on-chord"]
    restart_on_chord: BoolParam,
}

impl Default for ArpParams {
    fn default() -> Self {
        let octaves = |name| {
            IntParam::new(
                name,
                0,
                IntRange::Linear {
                    min: 0,
                    max: arp::MAX_OCTAVE_SHIFT as i32,
                },
            )
        };
        Self {
            shape: EnumParam::new("Shape", Shape::Stairs),
            direction: EnumParam::new("Direction", Direction::Up),
            start: EnumParam::new("Start", Start::Outside),
            edge: EnumParam::new("Edge", Edge::Restart),
            pair: EnumParam::new("Pair", Pair::Off),
            repeat_ends: BoolParam::new("Repeat Ends", false),
            length: IntParam::new("Length", 0, IntRange::Linear { min: 0, max: 32 })
                .with_value_to_string(Arc::new(|steps| match steps {
                    0 => "Full".into(),
                    steps => steps.to_string(),
                }))
                .with_string_to_value(Arc::new(|string| match string.trim() {
                    full if full.eq_ignore_ascii_case("full") => Some(0),
                    steps => steps.parse().ok(),
                })),
            notes: EnumParam::new("Notes", Notes::One),
            chord_velocity: FloatParam::new(
                "Chord Velocity",
                0.8,
                FloatRange::Linear {
                    min: 0.01,
                    max: 1.0,
                },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            rate: EnumParam::new("Rate", Rate::Sixteenth),
            note_length: FloatParam::new(
                "Note Length",
                1.0,
                FloatRange::Linear { min: 0.0, max: 2.0 },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            octaves_down: octaves("Octaves Down"),
            octaves_up: octaves("Octaves Up"),
            octave_behavior: EnumParam::new("Octave Behavior", OctaveBehavior::Thin),
            velocity_mode: EnumParam::new("Velocity Mode", VelocityMode::AsPlayed),
            velocity: IntParam::new("Fixed Velocity", 100, IntRange::Linear { min: 1, max: 127 }),
            advance: EnumParam::new("Advance", Advance::Tempo),
            trigger_channel: IntParam::new(
                "Trigger Channel",
                16,
                IntRange::Linear { min: 1, max: 16 },
            ),
            latch: BoolParam::new("Latch", false),
            restart_on_chord: BoolParam::new("Restart On Chord", false),
        }
    }
}

impl Default for StairsArp {
    fn default() -> Self {
        Self {
            params: Arc::new(ArpParams::default()),
            arp: Arp::default(),
        }
    }
}

impl StairsArp {
    fn settings(&self) -> Settings {
        let p = &self.params;
        Settings {
            pattern: Spec {
                shape: p.shape.value(),
                direction: p.direction.value(),
                start: p.start.value(),
                edge: p.edge.value(),
                pair: p.pair.value(),
                repeat_ends: p.repeat_ends.value(),
                octave_behavior: p.octave_behavior.value(),
            },
            step_beats: p.rate.value().beats(),
            octaves_down: p.octaves_down.value() as usize,
            octaves_up: p.octaves_up.value() as usize,
            length: p.length.value() as usize,
            notes: match p.notes.value() {
                Notes::One => 1,
                Notes::Two => 2,
                Notes::Three => 3,
                Notes::All => usize::MAX,
            },
            chord_velocity: p.chord_velocity.value(),
            note_length: p.note_length.value() as f64,
            velocity: match p.velocity_mode.value() {
                VelocityMode::AsPlayed => Velocity::AsPlayed,
                VelocityMode::Fixed => Velocity::Fixed(p.velocity.value() as f32 / 127.0),
                VelocityMode::FromTrigger => Velocity::FromTrigger,
            },
            triggered: p.advance.value() == Advance::Trigger,
            latch: p.latch.value(),
            restart_on_chord: p.restart_on_chord.value(),
        }
    }

    fn handle(
        &mut self,
        event: NoteEvent<()>,
        timing: u32,
        s: &Settings,
        context: &mut impl ProcessContext<Self>,
    ) {
        let mut emit = |out| {
            let _ = context.try_send_event(to_event(timing, out));
        };
        match event {
            NoteEvent::NoteOn {
                key: Key::Number(key),
                channel,
                velocity,
                ..
            } => {
                let note = Note {
                    key,
                    channel: channel.number().unwrap_or(0),
                    velocity,
                };
                let trigger_channel = self.params.trigger_channel.value() as u8 - 1;
                if s.triggered && note.channel == trigger_channel {
                    self.arp.trigger(note, s, &mut emit);
                } else {
                    self.arp.key_on(note);
                }
            }
            NoteEvent::NoteOff { key, channel, .. } | NoteEvent::Choke { key, channel, .. } => {
                // Both, whatever the channel: the note may predate a change of Advance or
                // Trigger Channel.
                self.arp.release(key.number(), channel.number(), &mut emit);
                self.arp.key_off(key.number(), channel.number(), s.latch);
            }
            // The arp replaces the notes; channel-wide messages pass straight through.
            NoteEvent::MidiCC { .. }
            | NoteEvent::MidiPitchBend { .. }
            | NoteEvent::MidiChannelPressure { .. }
            | NoteEvent::MidiProgramChange { .. } => {
                let _ = context.try_send_event(event);
            }
            _ => (),
        }
    }
}

fn to_event(timing: u32, out: Out) -> NoteEvent<()> {
    match out {
        Out::On(note) => NoteEvent::NoteOn {
            timing,
            voice_id: VoiceID::Wildcard,
            channel: Channel::Number(note.channel),
            key: Key::Number(note.key),
            velocity: note.velocity,
        },
        Out::Off { key, channel } => NoteEvent::NoteOff {
            timing,
            voice_id: VoiceID::Wildcard,
            channel: Channel::Number(channel),
            key: Key::Number(key),
            velocity: 0.0,
        },
    }
}

impl Plugin for StairsArp {
    const NAME: &'static str = "omni-arp";
    const VENDOR: &'static str = "Thomas Heartman";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // A note effect: no audio in or out.
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::MidiCCs;
    // Parameters are read once per block. Turning this on would also make nice-plug's CLAP
    // wrapper add the block offset to transport positions that are already current after a
    // transport event, which shifts the grid.
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type Editor = ();
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn reset(&mut self) {
        self.arp.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let s = self.settings();
        let transport = context.transport();
        let beats_per_sample =
            transport.tempo.unwrap_or(120.0) / 60.0 / transport.sample_rate as f64;
        // Without a song position there is no grid to sync to, so free-run as if stopped.
        let start_beat = transport.pos_beats().filter(|_| transport.playing);

        let len = buffer.samples() as u32;
        let mut next = context.next_event();
        for t in 0..len {
            while let Some(event) = next.filter(|e| e.timing() <= t) {
                self.handle(event, t, &s, context);
                next = context.next_event();
            }
            let beat = start_beat.map(|b| b + t as f64 * beats_per_sample);
            self.arp.tick(beat, beats_per_sample, &s, &mut |out| {
                let _ = context.try_send_event(to_event(t, out));
            });
        }
        // Events timed past the end of the buffer shouldn't exist, but a dropped note-off would
        // leave a key in the pool forever.
        while let Some(event) = next {
            self.handle(event, len.saturating_sub(1), &s, context);
            next = context.next_event();
        }

        // Keep getting called while notes play: there's no audio input for the host to wake us.
        if self.arp.is_idle() {
            ProcessStatus::Normal
        } else {
            ProcessStatus::KeepAlive
        }
    }
}

impl ClapPlugin for StairsArp {
    const CLAP_ID: &'static str = "com.thomasheartman.omni-arp";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Arpeggiator built from stairs, groups of three, mirrored walks and pedal notes");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::NoteEffect];

    fn remote_controls(&self, context: &mut impl RemoteControlsContext) {
        let p = &self.params;
        context.add_section("omni-arp", |section| {
            section.add_page("Pattern", |page| {
                page.add_param(&p.shape);
                page.add_param(&p.direction);
                page.add_param(&p.start);
                page.add_param(&p.edge);
                page.add_param(&p.pair);
                page.add_param(&p.repeat_ends);
                page.add_param(&p.length);
                page.add_param(&p.rate);
            });
            section.add_page("Notes", |page| {
                page.add_param(&p.notes);
                page.add_param(&p.chord_velocity);
                page.add_param(&p.note_length);
                page.add_param(&p.octaves_down);
                page.add_param(&p.octaves_up);
                page.add_param(&p.octave_behavior);
                page.add_param(&p.velocity_mode);
                page.add_param(&p.velocity);
            });
            section.add_page("Input", |page| {
                page.add_param(&p.advance);
                page.add_param(&p.trigger_channel);
                page.add_param(&p.latch);
                page.add_param(&p.restart_on_chord);
            });
        });
    }
}

nice_export_clap!(StairsArp);
