/// OSC 9001 agent notification events.
///
/// Agents emit these mid-execution to push status or messages without waiting
/// for an exit code.  Format: `\e]9001;<payload>\e\\`
///
/// Payloads:
/// - `status:awaiting_user` — agent is waiting for user input
/// - `status:success`       — agent completed successfully
/// - `status:error`         — agent encountered an error
/// - `notify:<message>`     — free-form notification message
#[derive(Debug, Clone, PartialEq)]
pub enum OscNotification {
    StatusAwaiting,
    StatusSuccess,
    StatusError,
    Notify(String),
}

impl OscNotification {
    pub fn parse(params: &str) -> Option<Self> {
        let rest = params.strip_prefix("9001;")?;
        if rest == "status:awaiting_user" {
            return Some(Self::StatusAwaiting);
        }
        if rest == "status:success" {
            return Some(Self::StatusSuccess);
        }
        if rest == "status:error" {
            return Some(Self::StatusError);
        }
        if let Some(msg) = rest.strip_prefix("notify:") {
            return Some(Self::Notify(msg.to_owned()));
        }
        None
    }
}

/// OSC 133 shell integration markers for prompt boundary detection.
#[derive(Debug, Clone, PartialEq)]
pub enum Osc133 {
    /// `\e]133;A\e\\` — prompt start
    PromptStart,
    /// `\e]133;B\e\\` — prompt end
    PromptEnd,
    /// `\e]133;C\e\\` — command start (pre-exec)
    CommandStart,
    /// `\e]133;D;%d\e\\` — command finished with exit code
    CommandFinished(i32),
}

impl Osc133 {
    pub fn parse(params: &str) -> Option<Self> {
        match params {
            "133;A" => Some(Self::PromptStart),
            "133;B" => Some(Self::PromptEnd),
            "133;C" => Some(Self::CommandStart),
            s if s.starts_with("133;D;") => {
                let code = s["133;D;".len()..].parse().ok()?;
                Some(Self::CommandFinished(code))
            }
            _ => None,
        }
    }
}
