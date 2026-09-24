//! `tf-tap` watches the PTY byte stream for the OSC sequences TermForge cares
//! about, without modifying the stream.
//!
//! The terminal engine still receives every byte. The tap only reports
//! *where* in a chunk an event ended, so the session can split its feed at
//! that offset and anchor the event to an exact grid line.
//!
//! Recognised sequences:
//!
//! | Sequence | Meaning |
//! |---|---|
//! | `OSC 133 ; A/B/C/D` | Semantic prompt marks (FinalTerm / FTCS) |
//! | `OSC 7 ; file://host/path` | Working directory |
//! | `OSC 9 ; 9 ; path` | Working directory (ConEmu / Windows Terminal) |
//! | `OSC 9 ; 4 ; state ; value` | Progress (ConEmu) |
//! | `OSC 9 ; text` | Notification (iTerm2) |
//! | `OSC 777 ; notify ; title ; body` | Notification (rxvt / Ghostty) |
//! | `OSC 99 ; meta ; payload` | Notification (kitty, single chunk subset) |
//!
//! The parser is a small byte-at-a-time state machine. It is resumable
//! across arbitrary chunk boundaries and bounded in memory: payloads longer
//! than [`MAX_OSC_LEN`] are discarded.

mod parse;

pub use parse::parse_osc;

/// Maximum OSC payload the tap will buffer. Longer sequences (for example
/// inline images) are ignored rather than buffered.
pub const MAX_OSC_LEN: usize = 4096;

/// A semantic prompt mark (`OSC 133`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// `A`: the prompt is about to be drawn.
    PromptStart,
    /// `B`: the prompt has ended and user input begins.
    InputStart,
    /// `C`: the command was submitted and output begins.
    CommandExecuted,
    /// `D`: the command finished. The exit code is optional in the spec.
    CommandFinished { exit_code: Option<i32> },
}

/// ConEmu progress state (`OSC 9 ; 4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    Clear,
    Normal,
    Error,
    Indeterminate,
    Paused,
}

/// Something the tap recognised in the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapEvent {
    Mark(Mark),
    Cwd(String),
    Notify {
        title: Option<String>,
        body: String,
    },
    Progress {
        state: ProgressState,
        percent: Option<u8>,
    },
}

/// A recognised event plus the offset, within the chunk passed to
/// [`Tap::feed`], of the byte immediately after the sequence terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub offset: usize,
    pub event: TapEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    OscBody,
    OscEscape,
    /// Payload exceeded [`MAX_OSC_LEN`]; skip until the terminator.
    OscOverflow,
    OscOverflowEscape,
}

/// Resumable OSC tap. Create one per PTY session.
#[derive(Debug)]
pub struct Tap {
    state: State,
    buf: Vec<u8>,
}

impl Default for Tap {
    fn default() -> Self {
        Self::new()
    }
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

impl Tap {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            buf: Vec::with_capacity(256),
        }
    }

    /// Scan `input` and append any recognised events to `out`.
    ///
    /// Returns the number of events appended.
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<Located>) -> usize {
        let before = out.len();
        for (i, &b) in input.iter().enumerate() {
            self.step(b, i + 1, out);
        }
        out.len() - before
    }

    fn step(&mut self, b: u8, offset: usize, out: &mut Vec<Located>) {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Escape;
                }
            }
            State::Escape => {
                self.state = match b {
                    b']' => {
                        self.buf.clear();
                        State::OscBody
                    }
                    ESC => State::Escape,
                    _ => State::Ground,
                };
            }
            State::OscBody => match b {
                BEL => self.finish(offset, out),
                ESC => self.state = State::OscEscape,
                CAN | SUB => self.abort(),
                _ => {
                    if self.buf.len() >= MAX_OSC_LEN {
                        self.buf.clear();
                        self.state = State::OscOverflow;
                    } else {
                        self.buf.push(b);
                    }
                }
            },
            State::OscEscape => {
                if b == b'\\' {
                    self.finish(offset, out);
                } else {
                    // ESC not followed by '\' cancels the string and starts a
                    // new escape sequence with this byte.
                    self.abort();
                    self.state = State::Escape;
                    self.step(b, offset, out);
                }
            }
            State::OscOverflow => match b {
                BEL | CAN | SUB => self.abort(),
                ESC => self.state = State::OscOverflowEscape,
                _ => {}
            },
            State::OscOverflowEscape => {
                if b == b'\\' {
                    self.abort();
                } else {
                    self.abort();
                    self.state = State::Escape;
                    self.step(b, offset, out);
                }
            }
        }
    }

    fn finish(&mut self, offset: usize, out: &mut Vec<Located>) {
        if let Some(event) = parse_osc(&self.buf) {
            out.push(Located { offset, event });
        }
        self.abort();
    }

    fn abort(&mut self) {
        self.buf.clear();
        self.state = State::Ground;
    }
}

#[cfg(test)]
mod tests;
