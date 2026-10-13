//! Client/account context resolution and Git identity checks.

use std::path::Path;
use std::process::{Command, Stdio};

use tf_config::ContextConfig;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityCheck {
    pub actual_email: Option<String>,
    pub origin_host: Option<String>,
    pub warnings: Vec<String>,
}

/// Replace context-controlled keys while preserving PATH and unrelated launch
/// environment. Existing shells are intentionally unaffected.
pub fn apply_to_env(env: &mut Vec<(String, String)>, context: Option<&ContextConfig>) {
    const KEYS: &[&str] = &[
        "TERMFORGE_CONTEXT",
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "GIT_CONFIG_GLOBAL",
        "GH_CONFIG_DIR",
    ];
    env.retain(|(key, _)| !KEYS.contains(&key.as_str()));
    if let Some(context) = context {
        env.extend(context.shell_env());
    }
}

pub fn inspect(root: Option<&Path>, context: Option<&ContextConfig>) -> IdentityCheck {
    let Some(root) = root else {
        return IdentityCheck::default();
    };
    let actual_email = git_value(root, &["config", "--get", "user.email"]);
    let origin = git_value(root, &["remote", "get-url", "origin"]);
    let origin_host = origin.as_deref().and_then(remote_host);
    let mut warnings = Vec::new();
    if let Some(context) = context {
        if !context.git_email.is_empty()
            && actual_email
                .as_deref()
                .is_none_or(|actual| !actual.eq_ignore_ascii_case(&context.git_email))
        {
            warnings.push(match &actual_email {
                Some(actual) => format!("Git email is {actual}, expected {}", context.git_email),
                None => format!("Git email is unset; expected {}", context.git_email),
            });
        }
        if !context.github_host.is_empty()
            && origin_host
                .as_deref()
                .is_some_and(|host| !host.eq_ignore_ascii_case(&context.github_host))
        {
            warnings.push(format!(
                "Origin host is {}, expected {}",
                origin_host.as_deref().unwrap_or_default(),
                context.github_host
            ));
        }
    }
    IdentityCheck {
        actual_email,
        origin_host,
        warnings,
    }
}

fn git_value(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Host from common HTTPS, SSH URL and SCP-like Git remote syntax.
fn remote_host(url: &str) -> Option<String> {
    let value = url.trim();
    if let Some((_, rest)) = value.split_once("://") {
        let authority = rest.split('/').next()?;
        return authority
            .rsplit('@')
            .next()
            .and_then(|host_port| host_port.split(':').next())
            .map(str::to_owned);
    }
    let before_colon = value.split(':').next()?;
    before_colon.rsplit('@').next().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_remote_hosts() {
        assert_eq!(
            remote_host("git@github.com:acme/app.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            remote_host("https://dev.azure.com/acme/app").as_deref(),
            Some("dev.azure.com")
        );
        assert_eq!(
            remote_host("ssh://git@gitlab.example:2222/team/app").as_deref(),
            Some("gitlab.example")
        );
    }

    #[test]
    fn switching_context_replaces_only_owned_environment() {
        let mut env = vec![
            ("PATH".into(), "bin".into()),
            ("GIT_AUTHOR_EMAIL".into(), "old@example.com".into()),
        ];
        let context = ContextConfig {
            git_email: "new@example.com".into(),
            ..ContextConfig::default()
        };
        apply_to_env(&mut env, Some(&context));
        assert!(env.contains(&("PATH".into(), "bin".into())));
        assert!(env.contains(&("GIT_AUTHOR_EMAIL".into(), "new@example.com".into())));
        assert!(!env.iter().any(|pair| pair.1 == "old@example.com"));
    }
}
