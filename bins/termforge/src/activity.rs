//! What happened, in order: a bounded activity log with filtering.
//!
//! Plain data with no GPUI or session types. Events are only ever recorded
//! from observed changes (a service changed state, a port opened, a container
//! stopped, the daemon connection changed), never invented.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::SystemTime;

/// Events kept; older ones are dropped first.
pub const CAPACITY: usize = 500;

/// Where an event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Daemon,
    Service,
    Port,
    Container,
    Language,
}

impl Source {
    pub const ALL: [Self; 5] = [
        Self::Daemon,
        Self::Service,
        Self::Port,
        Self::Container,
        Self::Language,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Daemon => "DAEMON",
            Self::Service => "SERVICE",
            Self::Port => "PORT",
            Self::Container => "CONTAINER",
            Self::Language => "LANGUAGE",
        }
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "all",
            Self::Warn => "warnings+",
            Self::Error => "errors",
        }
    }

    /// The next stricter level, wrapping back to showing everything.
    pub const fn cycle(self) -> Self {
        match self {
            Self::Info => Self::Warn,
            Self::Warn => Self::Error,
            Self::Error => Self::Info,
        }
    }
}

/// Where activating an event should go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// A workspace tab, by its id.
    Tab(u64),
    /// A source file and zero-based line.
    File(PathBuf, u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub at: SystemTime,
    pub source: Source,
    pub level: Level,
    pub message: String,
    pub link: Option<Link>,
}

#[derive(Debug, Default)]
pub struct ActivityLog {
    events: VecDeque<Event>,
}

impl ActivityLog {
    pub fn push(&mut self, event: Event) {
        self.events.push_back(event);
        while self.events.len() > CAPACITY {
            self.events.pop_front();
        }
    }

    pub fn record(&mut self, source: Source, level: Level, message: impl Into<String>) {
        self.push(Event {
            at: SystemTime::now(),
            source,
            level,
            message: message.into(),
            link: None,
        });
    }

    pub fn record_linked(
        &mut self,
        source: Source,
        level: Level,
        message: impl Into<String>,
        link: Link,
    ) {
        self.push(Event {
            at: SystemTime::now(),
            source,
            level,
            message: message.into(),
            link: Some(link),
        });
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Events passing `filter`, newest first.
    pub fn visible(&self, filter: &Filter) -> Vec<&Event> {
        self.events
            .iter()
            .rev()
            .filter(|event| filter.matches(event))
            .collect()
    }
}

/// What the Timeline shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    hidden: u8,
    pub min_level: Level,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            text: String::new(),
            hidden: 0,
            min_level: Level::Info,
        }
    }
}

impl Filter {
    pub fn shows(&self, source: Source) -> bool {
        self.hidden & source.bit() == 0
    }

    pub fn toggle(&mut self, source: Source) {
        self.hidden ^= source.bit();
    }

    pub fn is_active(&self) -> bool {
        !self.text.is_empty() || self.hidden != 0 || self.min_level != Level::Info
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Case-insensitive match against the message and the source name.
    pub fn matches(&self, event: &Event) -> bool {
        if !self.shows(event.source) || event.level < self.min_level {
            return false;
        }
        if self.text.is_empty() {
            return true;
        }
        let needle = self.text.to_lowercase();
        event.message.to_lowercase().contains(&needle)
            || event.source.label().to_lowercase().contains(&needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> ActivityLog {
        let mut log = ActivityLog::default();
        log.record(Source::Daemon, Level::Warn, "forged offline");
        log.record(Source::Service, Level::Error, "cargo run failed (101)");
        log.record(Source::Port, Level::Info, "port :8080 opened (python3)");
        log.record_linked(
            Source::Language,
            Level::Warn,
            "src/main.rs: unused variable",
            Link::File("src/main.rs".into(), 4),
        );
        log
    }

    #[test]
    fn newest_events_come_first() {
        let log = log();
        let all = log.visible(&Filter::default());
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].source, Source::Language);
        assert_eq!(all[3].source, Source::Daemon);
    }

    #[test]
    fn text_filter_ignores_case_and_matches_source_names() {
        let log = log();
        let mut filter = Filter {
            text: "FAILED".into(),
            ..Filter::default()
        };
        assert_eq!(log.visible(&filter).len(), 1);
        filter.text = "port".into();
        assert_eq!(log.visible(&filter).len(), 1, "message or source label");
        filter.text = "nothing like this".into();
        assert!(log.visible(&filter).is_empty());
    }

    #[test]
    fn sources_toggle_and_level_cycles() {
        let log = log();
        let mut filter = Filter::default();
        filter.toggle(Source::Port);
        assert!(!filter.shows(Source::Port));
        assert_eq!(log.visible(&filter).len(), 3);
        filter.toggle(Source::Port);
        assert!(filter.shows(Source::Port));

        filter.min_level = filter.min_level.cycle();
        assert_eq!(filter.min_level, Level::Warn);
        assert_eq!(log.visible(&filter).len(), 3, "info is hidden");
        filter.min_level = filter.min_level.cycle();
        assert_eq!(log.visible(&filter).len(), 1, "errors only");
        filter.min_level = filter.min_level.cycle();
        assert_eq!(filter.min_level, Level::Info);
    }

    #[test]
    fn active_state_and_clear() {
        let mut filter = Filter::default();
        assert!(!filter.is_active());
        filter.text = "x".into();
        assert!(filter.is_active());
        filter.clear();
        assert!(!filter.is_active());
        filter.toggle(Source::Daemon);
        assert!(filter.is_active());
    }

    #[test]
    fn the_log_is_bounded_and_drops_the_oldest() {
        let mut log = ActivityLog::default();
        for n in 0..CAPACITY + 25 {
            log.record(Source::Port, Level::Info, format!("event {n}"));
        }
        assert_eq!(log.len(), CAPACITY);
        let all = log.visible(&Filter::default());
        assert_eq!(all[0].message, format!("event {}", CAPACITY + 24));
        assert_eq!(all.last().unwrap().message, "event 25");
    }
}
