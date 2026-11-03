//! Virtual MIDI source for checking resident desktop control on real machines.
use std::{error::Error, thread, time::Duration};

use midir::{MidiOutput, MidiOutputConnection, os::unix::VirtualOutput as _};

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Note(u8),
    Cc(u8, u8),
    Script,
    Quit,
}

fn command(line: &str) -> Option<Command> {
    match line.trim() {
        "" => Some(Command::Script),
        "q" => Some(Command::Quit),
        "c7" => Some(Command::Cc(7, 0)),
        "c10" => Some(Command::Cc(10, 0)),
        "c11" => Some(Command::Cc(11, 64)),
        note => note
            .strip_prefix('n')?
            .parse::<u8>()
            .ok()
            .filter(|note| (60..=64).contains(note))
            .map(Command::Note),
    }
}

fn note(source: &mut MidiOutputConnection, number: u8) -> Result<(), midir::SendError> {
    source.send(&[0x90, number, 100])?;
    source.send(&[0x90, number, 0])
}

fn script(source: &mut MidiOutputConnection) -> Result<(), midir::SendError> {
    for number in [60, 61, 62, 63, 62, 64] {
        note(source, number)?;
        thread::sleep(Duration::from_millis(250));
    }
    for value in (0..=127).chain((0..127).rev()) {
        source.send(&[0xb0, 7, value])?;
        thread::sleep(Duration::from_millis(2));
    }
    for (number, values) in [(10, [1, 1, 127, 0]), (11, [65, 65, 63, 64])] {
        for value in values {
            source.send(&[0xb0, number, value])?;
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().any(|argument| argument == "--help") {
        println!(
            "MIDI lab: creates 'MixRelay MIDI Lab' using CoreMIDI/ALSA.\nFor learning, enter n60 power, n61 mute, n62 record/resume, n63 pause, n64 stop,\nc7 absolute fader, c10 two's complement, or c11 offset 64. All use channel 1.\nAn empty Enter sends the full note/CC script. q quits.\nThe source stays open between commands. No audio or sockets are opened."
        );
        return Ok(());
    }
    let output = MidiOutput::new("MixRelay MIDI Lab")?;
    let mut source = output.create_virtual("MixRelay MIDI Lab")?;
    println!("MixRelay MIDI Lab is ready. Learn: n60–n64 or c7/c10/c11. Enter: script. q: quit.");
    let mut line = String::new();
    loop {
        line.clear();
        if std::io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        match command(&line) {
            Some(Command::Quit) => break,
            Some(Command::Note(number)) => note(&mut source, number)?,
            Some(Command::Cc(number, value)) => source.send(&[0xb0, number, value])?,
            Some(Command::Script) => script(&mut source)?,
            None => {
                println!("Use n60–n64, c7, c10, c11, an empty Enter, or q.");
                continue;
            }
        }
        println!("Sent. Enter to run the script; q to quit.");
    }
    Ok(())
}

#[cfg(test)]
#[path = "midi_lab/tests.rs"]
mod tests;
