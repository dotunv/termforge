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
