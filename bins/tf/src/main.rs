//! `tf` is the CLI companion used inside TermForge terminals (and by agents
//! running in them).

use std::io::Write;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "tf", version, about = "TermForge CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Raise a desktop notification from inside a terminal.
    Notify {
        /// Notification body.
        body: String,
        #[arg(short, long)]
        title: Option<String>,
    },
    /// Print a shell-integration script, e.g. `tf shell-integration zsh`.
    ShellIntegration { shell: Shell },
    /// Print version information.
    Version,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Shell {
    Pwsh,
    Bash,
    Zsh,
    Fish,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut out = std::io::stdout().lock();
    match cli.command {
        Command::Notify { body, title } => {
            out.write_all(&notify_sequence(title.as_deref(), &body))?
        }
        Command::ShellIntegration { shell } => out.write_all(
            match shell {
                Shell::Pwsh => tf_shell::PWSH,
                Shell::Bash => tf_shell::BASH,
                Shell::Zsh => tf_shell::ZSH,
                Shell::Fish => tf_shell::FISH,
            }
            .as_bytes(),
        )?,
        Command::Version => writeln!(
            out,
            "tf {} (protocol v{})",
            env!("CARGO_PKG_VERSION"),
            tf_proto::PROTOCOL_VERSION
        )?,
    }
    out.flush()?;
    Ok(())
}

/// OSC 777 carries a title; control characters are stripped so user text
/// can never terminate the sequence early or inject escapes.
fn notify_sequence(title: Option<&str>, body: &str) -> Vec<u8> {
    let clean = |s: &str| {
        s.chars()
            .filter(|c| !c.is_control() && *c != ';')
            .collect::<String>()
    };
    let body: String = body.chars().filter(|c| !c.is_control()).collect();
    match title {
        Some(t) => format!("\x1b]777;notify;{};{}\x1b\\", clean(t), body).into_bytes(),
        None => format!("\x1b]9;{body}\x1b\\").into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tf_tap::{Tap, TapEvent};

    fn parse(bytes: &[u8]) -> Vec<TapEvent> {
        let mut out = Vec::new();
        Tap::new().feed(bytes, &mut out);
        out.into_iter().map(|l| l.event).collect()
    }

    #[test]
    fn notify_roundtrips_through_the_tap() {
        assert_eq!(
            parse(&notify_sequence(Some("Build"), "done; 3 warnings")),
            vec![TapEvent::Notify {
                title: Some("Build".into()),
                body: "done; 3 warnings".into()
            }]
        );
        assert_eq!(
            parse(&notify_sequence(None, "hello")),
            vec![TapEvent::Notify {
                title: None,
                body: "hello".into()
            }]
        );
    }

    #[test]
    fn control_characters_cannot_escape_the_sequence() {
        let seq = notify_sequence(Some("a\x07b"), "x\x1b]133;A\x07y");
        assert_eq!(parse(&seq).len(), 1);
    }
}
