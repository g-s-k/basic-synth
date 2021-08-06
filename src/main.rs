use std::{
    f32::consts::TAU,
    io::{stdin, stdout, Write},
    process,
    sync::mpsc::{self, Sender},
    thread,
};

use {
    midi_msg::*,
    midir::{Ignore, MidiInput},
    rodio::{buffer::SamplesBuffer, source::Source, OutputStream, Sink},
};

const SAMPLE_RATE: u32 = 44100;

fn main() {
    let mut midi_in = MidiInput::new("basic-synth").expect("Could not create MIDI Input object");
    midi_in.ignore(Ignore::None);

    let in_ports = midi_in.ports();
    let in_port = match in_ports.as_slice() {
        [] => {
            eprintln!("No MIDI ports available");
            process::exit(101);
        }
        [only_one] => only_one,
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
        .connect(in_port, "basic-synth-midi-in", process_midi, Synth::new(16))
        .expect("Failed to connect to MIDI source");

    let mut input = String::new();
    stdin().read_line(&mut input).unwrap();
}

fn process_midi(stamp: u64, message: &[u8], synth: &mut Synth) {
    let (msg, len) = MidiMsg::from_midi(message).expect("Bad MIDI data");
    match msg {
        MidiMsg::ChannelVoice {
            msg: ChannelVoiceMsg::NoteOn { note, velocity },
            ..
        } => {
            if let Some(v) = synth.get_new_voice() {
                v.on = true;
                v.note = note;
                v.tx.send(VoiceMessage::Start {
                    note,
                    vel: velocity,
                })
                .expect("Failed to send NoteOn message");
            } else {
                eprintln!(
                    "Out of voices. Note requested was {} with velocity {}",
                    note, velocity
                );
            }
        }
        MidiMsg::ChannelVoice {
            msg: ChannelVoiceMsg::NoteOff { note, .. },
            ..
        } => {
            if let Some(v) = synth.get_playing_voice(note) {
                v.on = false;
                v.tx.send(VoiceMessage::Stop)
                    .expect("Failed to send NoteOff message");
            } else {
                eprintln!(
                    "Expected a voice playing note {} but could not find one",
                    note
                );
            }
        }
        _ => {
            println!("{}: {:?} (len = {})", stamp, msg, len);
        }
    }
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

#[derive(Debug)]
struct Voice {
    on: bool,
    note: u8,
    tx: Sender<VoiceMessage>,
}

impl Default for Voice {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let (_stream, stream_handle) = OutputStream::try_default().unwrap();
            let sink = Sink::try_new(&stream_handle).unwrap();

            for msg in rx {
                match msg {
                    VoiceMessage::Start { note, vel } => {
                        let freq = midi_frequency(note);
                        let period = (1.0 / freq * SAMPLE_RATE as f32).round() as usize;
                        let freq_const = TAU * freq / SAMPLE_RATE as f32;

                        let buffer_ary: Vec<_> = (0..period)
                            .map(|samp| (freq_const * samp as f32).sin())
                            .collect();

                        sink.append(
                            SamplesBuffer::new(1, SAMPLE_RATE, buffer_ary)
                                .amplify(vel as f32 / 127.0)
                                .repeat_infinite(),
                        );
                    }
                    VoiceMessage::Stop => {
                        sink.stop();
                    }
                }
            }
        });

        Self {
            on: false,
            note: 0,
            tx,
        }
    }
}

#[derive(Debug)]
enum VoiceMessage {
    Start { note: u8, vel: u8 },
    Stop,
}

fn midi_frequency(note: u8) -> f32 {
    // A4 is note number 69 and has frequency 440
    (2_f32).powf((note as f32 - 69.0) / 12.0) * 440.0
}
