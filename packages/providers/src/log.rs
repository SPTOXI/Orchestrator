//! Session transcript: numbered events, bounded, with consecutive text
//! fragments merged (ADR-0009).

use chrono::Utc;
use orchestrator_core::{SessionEvent, SessionLogEntry};
use std::collections::VecDeque;

pub(crate) struct SessionLog {
    entries: VecDeque<SessionLogEntry>,
    next_seq: u64,
    capacity: usize,
    truncated: bool,
}

impl SessionLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            next_seq: 1,
            capacity: capacity.max(1),
            truncated: false,
        }
    }

    /// Appends an event and returns its sequence number.
    ///
    /// A text/reasoning fragment (or usage report) following one of the same
    /// kind and turn is merged into it; the merged entry takes the newest
    /// `seq`, so every fragment up to that number is contained in it.
    pub fn push(&mut self, event: SessionEvent) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if let Some(last) = self.entries.back_mut() {
            if merge(&mut last.event, &event) {
                last.seq = seq;
                last.at = Utc::now();
                return seq;
            }
        }
        self.entries.push_back(SessionLogEntry {
            seq,
            at: Utc::now(),
            event,
        });
        if self.entries.len() > self.capacity {
            self.entries.pop_front();
            self.truncated = true;
        }
        seq
    }

    pub fn entries(&self) -> Vec<SessionLogEntry> {
        self.entries.iter().cloned().collect()
    }

    /// Highest sequence number handed out (0 when empty).
    pub fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// True when old entries were dropped to respect the capacity.
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

fn merge(last: &mut SessionEvent, next: &SessionEvent) -> bool {
    match (last, next) {
        (
            SessionEvent::TextDelta { turn_id: a, text },
            SessionEvent::TextDelta {
                turn_id: b,
                text: more,
            },
        )
        | (
            SessionEvent::ReasoningDelta { turn_id: a, text },
            SessionEvent::ReasoningDelta {
                turn_id: b,
                text: more,
            },
        ) if a == b => {
            text.push_str(more);
            true
        }
        (
            SessionEvent::Usage { turn_id: a, usage },
            SessionEvent::Usage {
                turn_id: b,
                usage: more,
            },
        ) if a == b => {
            *usage += *more;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_core::{SessionStatus, TokenUsage, TurnId};

    fn text(turn: &str, text: &str) -> SessionEvent {
        SessionEvent::TextDelta {
            turn_id: TurnId::from(turn),
            text: text.into(),
        }
    }

    #[test]
    fn merges_consecutive_text_of_the_same_turn() {
        let mut log = SessionLog::new(10);
        assert_eq!(log.push(text("t1", "Ol")), 1);
        assert_eq!(log.push(text("t1", "á")), 2);
        assert_eq!(log.push(text("t2", "!")), 3);
        let entries = log.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 2);
        assert_eq!(entries[0].event, text("t1", "Olá"));
        assert_eq!(log.last_seq(), 3);
    }

    #[test]
    fn merges_usage_reports() {
        let mut log = SessionLog::new(10);
        let usage = |n| SessionEvent::Usage {
            turn_id: TurnId::from("t"),
            usage: TokenUsage {
                input_tokens: n,
                ..Default::default()
            },
        };
        log.push(usage(2));
        log.push(usage(3));
        assert_eq!(log.entries()[0].event, usage(5));
    }

    #[test]
    fn drops_oldest_entries_beyond_capacity() {
        let mut log = SessionLog::new(2);
        for status in [
            SessionStatus::Running,
            SessionStatus::Idle,
            SessionStatus::Closed,
        ] {
            log.push(SessionEvent::StatusChanged { status });
        }
        let entries = log.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 2);
        assert!(log.truncated());
    }
}
