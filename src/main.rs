use std::{
    f32::consts::TAU,
    io::{stdin, stdout, Write},
    mem, process,
    sync::mpsc::{self, Sender, TryRecvError},
    thread,
};

use {
    midi_msg::*,
    midir::{Ignore, MidiInput},
    rodio::{buffer::SamplesBuffer, OutputStream, Sink},
};

const SAMPLE_RATE: u32 = 44100;
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
                    if sink.len() < 5 {
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
                    if let Some(v) = synth.get_new_voice() {
                        v.begin_note(note);
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
        Self {
            voices: (0..voices).map(|_| Voice::default()).collect(),
        }
    }

    fn get_new_voice(&mut self) -> Option<&mut Voice> {
        for voice in &mut self.voices {
            if !voice.on {
                return Some(voice);
            }
        }

        None
    }

    fn get_playing_voice(&mut self, note: u8) -> Option<&mut Voice> {
        for voice in &mut self.voices {
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
        Some(self.voices.iter_mut().flat_map(|v| v.next()).sum())
    }
}

#[derive(Debug, Default)]
struct Voice {
    on: bool,
    note: u8,
    oscillators: [Oscillator; 3],
    amp_eg: Adsr,
}

impl Voice {
    fn begin_note(&mut self, new_note: u8) {
        self.on = true;
        self.note = new_note;
        for osc in &mut self.oscillators {
            osc.current_freq = (2_f32).powf((self.note as i16 - 69) as f32 / 12.0) * 440.0;
        }
        self.amp_eg.segment = AdsrSegment::Attack(0.0);
    }

    fn end_note(&mut self) {
        self.on = false;
        self.amp_eg.segment = AdsrSegment::Release(0.0);
    }
}

impl Iterator for Voice {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let osc_mix: f32 = self.oscillators.iter_mut().flat_map(|o| o.next()).sum();
        let amp_volume = self.amp_eg.next().unwrap();
        Some(osc_mix * amp_volume)
    }
}

#[derive(Debug, Default)]
struct Oscillator {
    current_phase: f32,
    current_freq: f32,
}

impl Iterator for Oscillator {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let next_phase = (self.current_phase + TAU * self.current_freq / SAMPLE_RATE as f32) % TAU;
        Some(mem::replace(&mut self.current_phase, next_phase).sin())
    }
}

#[derive(Debug)]
struct Adsr {
    segment: AdsrSegment,
    attack_time: f32,
    decay_time: f32,
    sustain_amount: f32,
    release_time: f32,
}

impl Default for Adsr {
    fn default() -> Self { 
        Self {
            segment: AdsrSegment::Off,
            attack_time: 0.05,
            decay_time: 0.25,
            sustain_amount: 0.5,
            release_time: 0.5,
        }
    }
}

impl Iterator for Adsr {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.segment {
            AdsrSegment::Off => 0.0,
            AdsrSegment::Attack(amt) if amt >= 1.0 => {
                self.segment = AdsrSegment::Decay(0.0);
                1.0
            }
            AdsrSegment::Attack(current_amt) => {
                self.segment = AdsrSegment::Attack(current_amt + self.attack_time / SAMPLE_RATE as f32);
                // (y2 - y1) = m * (x2 - x1)
                // (y2 - 0.0) = (1.0 / attack) * (x2 - 0.0)
                // y2 = x2 / attack
                current_amt / self.attack_time
            }
            AdsrSegment::Decay(amt) if amt >= 1.0 => {
                self.segment = AdsrSegment::Sustain;
                self.sustain_amount
            }
            AdsrSegment::Decay(current_amt) => {
                self.segment = AdsrSegment::Decay(current_amt + self.decay_time / SAMPLE_RATE as f32);
                // (y2 - y1) = m * (x2 - x1)
                // (y2 - 1.0) = (1 - sustain) / 1.0 * (x2 - 0.0)
                // y2 = (1 - sustain) * x2 + 1.0
                (1.0 - self.sustain_amount) * current_amt + 1.0
            }
            AdsrSegment::Sustain => self.sustain_amount,
            AdsrSegment::Release(amt) if amt >= 1.0 => {
                self.segment = AdsrSegment::Off;
                0.0
            }
            AdsrSegment::Release(current_amt) => {
                self.segment = AdsrSegment::Release(current_amt + self.release_time / SAMPLE_RATE as f32);
                // (y2 - y1) = m * (x2 - x1)
                // (y2 - sustain) = (-sustain) * (x2 - 0.0)
                // y2 = (-sustain) * x2 + sustain
                // y2 = sustain * (1.0 - x2)
                self.sustain_amount * (1.0 - current_amt)
            }
        })
    }
}

#[derive(Debug)]
enum AdsrSegment {
    Off,
    Attack(f32),
    Decay(f32),
    Sustain,
    Release(f32),
}
