use std::{
    io::{stdin, stdout, Write},
    process,
};

use {
    midi_msg::*,
    midir::{Ignore, MidiInput},
    rodio::{buffer::SamplesBuffer, source::Source, OutputStream, OutputStreamHandle, StreamError},
};

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
        .connect(
            in_port,
            "basic-synth-midi-in",
            process_midi,
            Synth::<16>::new().expect("Could not initialize synth"),
        )
        .expect("Failed to connect to MIDI source");

    let mut input = String::new();
    stdin().read_line(&mut input).unwrap();
}

fn process_midi<const N: usize>(stamp: u64, message: &[u8], synth: &mut Synth<N>) {
    let (msg, len) = MidiMsg::from_midi(message).expect("Bad MIDI data");
    match msg {
        MidiMsg::ChannelVoice {
            msg: ChannelVoiceMsg::NoteOn { note, velocity },
            ..
        } => {
            if let Some(v) = synth.get_new_voice() {
                v.on = true;
                v.note = note;
                v.vel = velocity;
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
                v.vel = 0;
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

struct Synth<const N: usize> {
    voices: [Voice; N],
    output: OutputStreamHandle,
}

impl<const N: usize> Synth<N> {
    fn new() -> Result<Self, StreamError> {
        Ok(Self {
            voices: [Default::default(); N],
            output: OutputStream::try_default()?.1,
        })
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

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    on: bool,
    note: u8,
    vel: u8,
}
