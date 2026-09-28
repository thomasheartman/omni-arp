use nice_plug::{
    midi::{Channel, Key, VoiceID},
    prelude::*,
};
use std::sync::Arc;

mod arp;
mod pattern;

use arp::{Arp, Note, Out, Settings};
use pattern::Mode;

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
enum VelocityMode {
    #[id = "played"]
    #[name = "As Played"]
    AsPlayed,
    #[id = "fixed"]
    Fixed,
}

#[derive(Params)]
struct ArpParams {
    #[id = "mode"]
    mode: EnumParam<Mode>,
    #[id = "rate"]
    rate: EnumParam<Rate>,
    #[id = "octaves"]
    octaves: IntParam,
    #[id = "gate"]
    gate: FloatParam,
    #[id = "velocity-mode"]
    velocity_mode: EnumParam<VelocityMode>,
    #[id = "velocity"]
    velocity: IntParam,
    #[id = "latch"]
    latch: BoolParam,
    #[id = "repeat-ends"]
    repeat_ends: BoolParam,
    #[id = "restart-on-chord"]
    restart_on_chord: BoolParam,
}

impl Default for ArpParams {
    fn default() -> Self {
        Self {
            mode: EnumParam::new("Mode", Mode::StairsUp),
            rate: EnumParam::new("Rate", Rate::Sixteenth),
            octaves: IntParam::new(
                "Octaves",
                1,
                IntRange::Linear {
                    min: 1,
                    max: arp::MAX_OCTAVES as i32,
                },
            ),
            gate: FloatParam::new(
                "Gate",
                0.5,
                FloatRange::Linear {
                    min: 0.01,
                    max: 1.0,
                },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            velocity_mode: EnumParam::new("Velocity Mode", VelocityMode::AsPlayed),
            velocity: IntParam::new("Fixed Velocity", 100, IntRange::Linear { min: 1, max: 127 }),
            latch: BoolParam::new("Latch", false),
            repeat_ends: BoolParam::new("Repeat Ends", false),
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
            mode: p.mode.value(),
            step_beats: p.rate.value().beats(),
            octaves: p.octaves.value() as usize,
            gate: p.gate.value() as f64,
            velocity: (p.velocity_mode.value() == VelocityMode::Fixed)
                .then(|| p.velocity.value() as f32 / 127.0),
            latch: p.latch.value(),
            repeat_ends: p.repeat_ends.value(),
            restart_on_chord: p.restart_on_chord.value(),
        }
    }

    fn handle(
        &mut self,
        event: NoteEvent<()>,
        context: &mut impl ProcessContext<Self>,
        latch: bool,
    ) {
        match event {
            NoteEvent::NoteOn {
                key: Key::Number(key),
                channel,
                velocity,
                ..
            } => self.arp.key_on(Note {
                key,
                channel: channel.number().unwrap_or(0),
                velocity,
            }),
            NoteEvent::NoteOff { key, .. } | NoteEvent::Choke { key, .. } => {
                self.arp.key_off(key.number(), latch)
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

        let mut next = context.next_event();
        for t in 0..buffer.samples() as u32 {
            while let Some(event) = next.filter(|e| e.timing() <= t) {
                self.handle(event, context, s.latch);
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
            self.handle(event, context, s.latch);
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
        Some("Arpeggiator with Omnisphere 3's note patterns");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::NoteEffect];
}

nice_export_clap!(StairsArp);
