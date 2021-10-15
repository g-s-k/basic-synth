use std::{
    io::{stdin, stdout, Write},
    process,
    sync::mpsc::{self, Sender, TryRecvError},
    thread,
};

use {
    midi_msg::*,
    midir::{Ignore, MidiInput},
    rodio::{buffer::SamplesBuffer, OutputStream, Sink},
};

use basic_synth::{Synth, BLOCK_SIZE, SAMPLE_RATE};

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
                    if let Err(_) = synth.try_begin_note(note, velocity) {
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
                    if let Err(_) = synth.try_end_note(note) {
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
