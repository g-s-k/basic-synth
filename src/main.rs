use std::{
    f32::consts::{PI, TAU},
    io::{stdin, stdout, Write},
    mem, ops, process,
    rc::Rc,
    sync::mpsc::{self, Sender, TryRecvError},
    thread, time,
};

use {
    midi_msg::*,
    midir::{Ignore, MidiInput},
    rodio::{buffer::SamplesBuffer, OutputStream, Sink},
};

const SAMPLE_RATE: u32 = 48000;
const OVERSAMPLE_RATIO: u32 = 4;
const OVERSAMPLE_RATE: u32 = SAMPLE_RATE * OVERSAMPLE_RATIO;
const BLOCKS_PER_SECOND: u32 = 100;
const BLOCK_SIZE: u32 = SAMPLE_RATE / BLOCKS_PER_SECOND;

fn main() {
    let mut midi_in = MidiInput::new("basic-synth").expect("Could not create MIDI Input object");
    midi_in.ignore(Ignore::None);

    let in_ports = midi_in.ports();
    let in_port = match in_ports.as_slice() {
        [] => {
            eprintln!("No MIDI ports available");
            process::exit(101);
        }
        [only_one] => {
            eprintln!(
                "Connecting to MIDI port: {}",
                midi_in.port_name(only_one).unwrap()
            );
            only_one
        }
        otherwise => {
            println!("More than one MIDI port is available:");
            for (i, p) in otherwise.iter().enumerate() {
                println!("\t{}: {}", i, midi_in.port_name(p).unwrap());
            }
            print!("Please select input port: ");
            stdout().flush().unwrap();
            let mut input = String::new();
            stdin().read_line(&mut input).unwrap();
            otherwise
                .get(
                    input
                        .trim()
                        .parse::<usize>()
                        .expect("Input was not an integer"),
                )
                .expect("Selected index is out of range")
        }
    };

    let _conn_in = midi_in
        .connect(in_port, "basic-synth-midi-in", process_midi, run_synth_bg())
        .expect("Failed to connect to MIDI source");

    let mut input = String::new();
    stdin().read_line(&mut input).unwrap();
}

fn process_midi(_stamp: u64, message: &[u8], tx: &mut Sender<MidiMsg>) {
    let (msg, _len) = MidiMsg::from_midi(message).expect("Bad MIDI data");
    tx.send(msg)
        .expect("Failed to send message to synth thread");
}

fn run_synth_bg() -> Sender<MidiMsg> {
    let (tx, rx) = mpsc::channel::<MidiMsg>();

    thread::spawn(move || {
        let mut synth = Synth::new(8);
        let (_stream, stream_handle) = OutputStream::try_default().unwrap();
        let sink = Sink::try_new(&stream_handle).unwrap();

        loop {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    // don't get ahead of ourselves
                    if sink.len() < 4 {
                        let buffer: Vec<f32> = (0..BLOCK_SIZE).flat_map(|_| synth.next()).collect();
                        sink.append(SamplesBuffer::new(1, SAMPLE_RATE, buffer));
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    panic!(
                        "Synth thread disconnected from main thread unexpectedly. Shutting down."
                    );
                }
                Ok(MidiMsg::ChannelVoice {
                    msg: ChannelVoiceMsg::NoteOn { note, velocity },
                    ..
                }) => {
                    if let Some(v) = synth.get_playing_voice(note) {
                        v.begin_note(note, velocity);
                    } else if let Some(v) = synth.get_new_voice() {
                        v.begin_note(note, velocity);
                    } else {
                        eprintln!(
                            "Out of voices. Note requested was {} with velocity {}",
                            note, velocity
                        );
                    }
                }
                Ok(MidiMsg::ChannelVoice {
                    msg: ChannelVoiceMsg::NoteOff { note, .. },
                    ..
                }) => {
                    if let Some(v) = synth.get_playing_voice(note) {
                        v.end_note();
                    } else {
                        eprintln!(
                            "Expected a voice playing note {} but could not find one",
                            note
                        );
                    }
                }
                Ok(other_msg) => {
                    println!("{:?}", other_msg);
                }
            }
        }
    });

    tx
}

struct Synth {
    voices: Vec<Voice>,
}

impl Synth {
    fn new(voices: usize) -> Self {
        let amp_env_config = Rc::new(AdsrConfig::default());
        Self {
            voices: (0..voices)
                .map(move |_| Voice::new(amp_env_config.clone()))
                .collect(),
        }
    }

    fn get_new_voice(&mut self) -> Option<&mut Voice> {
        for voice in &mut self.voices {
            voice.check_note_done();
            if !voice.on {
                return Some(voice);
            }
        }

        None
    }

    fn get_playing_voice(&mut self, note: u8) -> Option<&mut Voice> {
        for voice in &mut self.voices {
            voice.check_note_done();
            if voice.on && voice.note == note {
                return Some(voice);
            }
        }

        None
    }
}

impl Iterator for Synth {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        Some(
            (0..OVERSAMPLE_RATIO)
                .map(|_| {
                    self.voices
                        .iter_mut()
                        .flat_map(|v| v.next())
                        .map(|v| (v * 0.75).min(1.0))
                        .sum::<f32>()
                })
                .nth(0)
                .unwrap(),
        )
    }
}

#[derive(Debug)]
struct Voice {
    on: bool,
    note: u8,
    detune: u8,
    oscillators: [Oscillator; 3],
    amp_eg: Adsr,
}

impl Voice {
    fn new(amp_env_config: Rc<AdsrConfig>) -> Self {
        Self {
            on: false,
            note: 0,
            detune: 5,
            oscillators: Default::default(),
            amp_eg: Adsr::new(amp_env_config),
        }
    }

    fn begin_note(&mut self, new_note: u8, new_vel: u8) {
        self.on = true;
        self.note = new_note;
        let detune_amount = self.detune as f32 / 100.0;
        let num_oscs = self.oscillators.len() as f32;
        for (index, osc) in self.oscillators.iter_mut().enumerate() {
            let note_plus_detune = self.note as f32
                + map_range(
                    index as f32,
                    (0.0, num_oscs),
                    (-detune_amount, detune_amount),
                );
            osc.current_freq = (2_f32).powf((note_plus_detune - 69.0) / 12.0) * 440.0;
        }
        self.amp_eg.segment = AdsrSegment::Attack(0.0, self.amp_eg.next().unwrap());
        self.amp_eg.velocity_ratio = new_vel as f32 / 127.0;
    }

    fn end_note(&mut self) {
        let release_point = if let AdsrSegment::Sustain = self.amp_eg.segment {
            self.amp_eg.config.sustain_amount
        } else {
            self.amp_eg.next().unwrap()
        };
        self.amp_eg.segment = AdsrSegment::Release(0.0, release_point);
    }

    fn check_note_done(&mut self) {
        if let AdsrSegment::Off = self.amp_eg.segment {
            self.on = false;
        }
    }
}

impl Iterator for Voice {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let osc_mix: f32 = self
            .oscillators
            .iter_mut()
            .flat_map(|o| o.next())
            .sum::<f32>()
            / (self.oscillators.len() as f32);
        let amp_volume = self.amp_eg.next().unwrap();
        Some(osc_mix * amp_volume)
    }
}

#[derive(Debug)]
struct Oscillator {
    current_phase: f32,
    current_freq: f32,
    wave: Waveform,
}

impl Default for Oscillator {
    fn default() -> Self {
        Self {
            current_phase: (time::SystemTime::now()
                .duration_since(time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                % 360) as f32,
            current_freq: 0.0,
            wave: Waveform::Saw,
        }
    }
}

impl Iterator for Oscillator {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let next_phase =
            (self.current_phase + TAU * self.current_freq / OVERSAMPLE_RATE as f32) % TAU;
        Some(
            self.wave
                .sample(mem::replace(&mut self.current_phase, next_phase)),
        )
    }
}

#[derive(Debug)]
enum Waveform {
    Sine,
    Pulse,
    Saw,
}

impl Waveform {
    fn sample(&self, phase: f32) -> f32 {
        match self {
            Self::Sine => phase.sin(),
            Self::Pulse if phase < PI => 1.0,
            Self::Pulse => -1.0,
            Self::Saw => (phase / PI) - 1.0,
        }
    }
}

#[derive(Debug)]
struct Adsr {
    config: Rc<AdsrConfig>,
    segment: AdsrSegment,
    velocity_ratio: f32,
}

impl Adsr {
    fn new(config: Rc<AdsrConfig>) -> Self {
        Self {
            config,
            segment: AdsrSegment::Off,
            velocity_ratio: 0.0,
        }
    }
}

impl Iterator for Adsr {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let raw_amplitude = match self.segment {
            AdsrSegment::Off => 0.0,
            AdsrSegment::Attack(amt, _) if amt >= 1.0 => {
                self.segment = AdsrSegment::Decay(0.0);
                1.0
            }
            AdsrSegment::Attack(current_amt, start_point) => {
                // velocity scaling - TODO use an actual mod matrix instead of hard coding
                let vel_scaled_attack_slope = 1.0
                    / map_range(
                        self.velocity_ratio,
                        (0.0, 1.0),
                        (self.config.attack_time, 0.0),
                    );
                self.segment = AdsrSegment::Attack(
                    current_amt + vel_scaled_attack_slope / OVERSAMPLE_RATE as f32,
                    start_point,
                );
                map_range(current_amt, (0.0, 1.0), (start_point, 1.0))
            }
            AdsrSegment::Decay(amt) if amt >= 1.0 => {
                self.segment = AdsrSegment::Sustain;
                self.config.sustain_amount
            }
            AdsrSegment::Decay(current_amt) => {
                // velocity scaling - TODO use an actual mod matrix instead of hard coding
                let vel_scaled_decay_slope = 1.0
                    / map_range(
                        self.velocity_ratio,
                        (0.0, 1.0),
                        (self.config.decay_time, 0.0),
                    );
                self.segment = AdsrSegment::Decay(
                    current_amt + vel_scaled_decay_slope / OVERSAMPLE_RATE as f32,
                );
                map_range(current_amt, (0.0, 1.0), (1.0, self.config.sustain_amount))
            }
            AdsrSegment::Sustain => self.config.sustain_amount,
            AdsrSegment::Release(amt, _) if amt >= 1.0 => {
                self.segment = AdsrSegment::Off;
                0.0
            }
            AdsrSegment::Release(current_amt, release_point) => {
                // velocity scaling - TODO use an actual mod matrix instead of hard coding
                let vel_scaled_release_slope = 1.0
                    / map_range(
                        self.velocity_ratio,
                        (0.0, 1.0),
                        (self.config.release_time, 0.0),
                    );
                self.segment = AdsrSegment::Release(
                    current_amt + vel_scaled_release_slope / OVERSAMPLE_RATE as f32,
                    release_point,
                );
                map_range(current_amt, (0.0, 1.0), (release_point, 0.0))
            }
        };

        // velocity scaling - TODO make the depth changeable via CC
        Some(raw_amplitude * map_range(self.velocity_ratio, (0.0, 1.0), (0.25, 1.0)))
    }
}

#[derive(Debug)]
struct AdsrConfig {
    attack_time: f32,
    decay_time: f32,
    sustain_amount: f32,
    release_time: f32,
}

impl Default for AdsrConfig {
    fn default() -> Self {
        Self {
            attack_time: 0.5,
            decay_time: 0.5,
            sustain_amount: 0.5,
            release_time: 1.0,
        }
    }
}

#[derive(Debug)]
enum AdsrSegment {
    Off,
    Attack(f32, f32),
    Decay(f32),
    Sustain,
    Release(f32, f32),
}

/// Transform a value from one range into another, relative to those ranges' limits.
///
/// To obtain an inversed relationship, put the "new" range in backward (from top to bottom).
fn map_range<T>(quantity: T, (bottom_old, top_old): (T, T), (bottom_new, top_new): (T, T)) -> T
where
    T: Copy
        + ops::Add<Output = T>
        + ops::Sub<Output = T>
        + ops::Div<Output = T>
        + ops::Mul<Output = T>,
{
    let rel_qty = (quantity - bottom_old) / (top_old - bottom_old);
    rel_qty * (top_new - bottom_new) + bottom_new
}
