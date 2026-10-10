//! `tf` is the CLI companion used inside TermForge terminals (and by agents
//! running in them).

use std::io::Write;
use std::path::PathBuf;

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
    /// Report durable program state to terminals that support OSC 7501.
    Status {
        #[arg(value_enum)]
        state: StatusState,
        #[arg(short, long)]
        message: Option<String>,
        #[arg(long)]
        app: Option<String>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long, value_enum)]
        kind: Option<StatusKind>,
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        progress: Option<u8>,
    },
    /// Manage durable project tasks through the local session host.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StatusState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    Clear,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StatusKind {
    Permission,
    Question,
    Auth,
}

#[derive(Debug, Subcommand)]
enum TaskCommand {
    List {
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    Create {
        title: String,
        #[arg(long, default_value = "")]
        context: String,
        /// A one-line shell command TermForge can run for this task.
        #[arg(long)]
        command: Option<String>,
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    Update {
        id: tf_proto::TaskId,
        #[arg(long, value_enum)]
        state: Option<TaskStateArg>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        context: Option<String>,
        /// Set the task's command; pass an empty string to clear it.
        #[arg(long)]
        command: Option<String>,
    },
    /// Delete a task.
    Delete { id: tf_proto::TaskId },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TaskStateArg {
    Planned,
    Active,
    Blocked,
    Done,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut out = std::io::stdout().lock();
    match cli.command {
        Command::Notify { body, title } => {
            out.write_all(&notify_sequence(title.as_deref(), &body))?
        }
        Command::Status {
            state,
            message,
            app,
            id,
            kind,
            progress,
        } => out.write_all(&status_sequence(
            state,
            message.as_deref(),
            app.as_deref(),
            id.as_deref(),
            kind,
            progress,
        )?)?,
        Command::Task { command } => run_task(command, &mut out)?,
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

fn run_task(command: TaskCommand, out: &mut impl Write) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let token = tf_ipc::token::ensure_token()?;
        let mut client = tf_ipc::IpcClient::connect("", &token, "tf-cli").await?;
        match command {
            TaskCommand::List { project } => {
                let project_root = dunce::canonicalize(project)?;
                let tf_proto::Response::Tasks(tasks) = client
                    .request(tf_proto::Request::ListTasks { project_root })
                    .await?
                else {
                    anyhow::bail!("unexpected response from forged");
                };
                for task in tasks {
                    writeln!(out, "{}\t{:?}\t{}", task.id, task.state, task.title)?;
                    if let Some(command) = &task.command {
                        writeln!(out, "  $ {command}")?;
                    }
                    if !task.context.is_empty() {
                        writeln!(out, "  {}", task.context)?;
                    }
                }
            }
            TaskCommand::Create {
                title,
                context,
                command,
                project,
            } => {
                let project_root = dunce::canonicalize(project)?;
                let response = client
                    .request(tf_proto::Request::CreateTask(tf_proto::CreateTask {
                        project_root,
                        title,
                        context,
                        command,
                    }))
                    .await?;
                let tf_proto::Response::TaskCreated(id) = response else {
                    anyhow::bail!("unexpected response from forged");
                };
                writeln!(out, "{id}")?;
            }
            TaskCommand::Update {
                id,
                state,
                title,
                context,
                command,
            } => {
                let state = state.map(|state| match state {
                    TaskStateArg::Planned => tf_proto::TaskState::Planned,
                    TaskStateArg::Active => tf_proto::TaskState::Active,
                    TaskStateArg::Blocked => tf_proto::TaskState::Blocked,
                    TaskStateArg::Done => tf_proto::TaskState::Done,
                });
                client
                    .request(tf_proto::Request::UpdateTask(tf_proto::UpdateTask {
                        id,
                        state,
                        title,
                        context,
                        command,
                    }))
                    .await?;
            }
            TaskCommand::Delete { id } => {
                client.request(tf_proto::Request::DeleteTask { id }).await?;
            }
        }
        Ok(())
    })
}

fn status_sequence(
    state: StatusState,
    message: Option<&str>,
    app: Option<&str>,
    id: Option<&str>,
    kind: Option<StatusKind>,
    progress: Option<u8>,
) -> Result<Vec<u8>> {
    let state = match state {
        StatusState::Idle => "idle",
        StatusState::Working => "working",
        StatusState::Done => "done",
        StatusState::Blocked => "blocked",
        StatusState::Error => "error",
        StatusState::Clear => "clear",
    };
    let mut pairs = vec![format!("state={state}")];
    if let Some(id) = id {
        anyhow::ensure!(valid_status_id(id), "invalid OSC 7501 record id");
        pairs.push(format!("id={id}"));
    }
    if let Some(app) = app {
        anyhow::ensure!(valid_status_name(app), "invalid OSC 7501 app name");
        pairs.push(format!("app={app}"));
    }
    if let Some(kind) = kind {
        anyhow::ensure!(state == "blocked", "--kind requires the blocked state");
        let kind = match kind {
            StatusKind::Permission => "permission",
            StatusKind::Question => "question",
            StatusKind::Auth => "auth",
        };
        pairs.push(format!("kind={kind}"));
    }
    if let Some(progress) = progress {
        anyhow::ensure!(
            matches!(state, "working" | "blocked"),
            "--progress requires working or blocked state"
        );
        pairs.push(format!("progress={progress}"));
    }
    if let Some(message) = message {
        anyhow::ensure!(
            !message.chars().any(char::is_control),
            "status messages cannot contain control characters"
        );
        let encoded = encode_base64(message.as_bytes());
        anyhow::ensure!(encoded.len() <= 2732, "status message is too long");
        pairs.push(format!("msg={encoded}"));
    }
    let sequence = format!("\x1b]7501;{}\x1b\\", pairs.join(":"));
    anyhow::ensure!(sequence.len() <= 4096, "status report is too long");
    Ok(sequence.into_bytes())
}

fn valid_status_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}

fn valid_status_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.split('/').count() <= 8
        && value.split('/').all(valid_status_name)
}

fn encode_base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[((bits >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((bits >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    out
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

    #[test]
    fn status_roundtrips_through_the_tap() {
        let seq = status_sequence(
            StatusState::Blocked,
            Some("Approve deploy?"),
            Some("tf"),
            Some("deploy/prod"),
            Some(StatusKind::Permission),
            Some(75),
        )
        .unwrap();
        let parsed = parse(&seq);
        assert!(matches!(parsed.as_slice(), [TapEvent::ProgramStatus(_)]));
    }
}
