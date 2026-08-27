use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::config::{AutoApprove, Limits};

/// What the broker should do with a request before any human sees it.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Refused by the deny list; never runs, never worth retrying.
    Blocked(String),
    /// Covered by the allow list, so the prompt is skipped.
    Allowed,
    /// Needs a human.
    Ask,
}

/// Glob match supporting `*` only, which is what autoapprove patterns need.
pub fn matches(pattern: &str, command: &str) -> bool {
    let command = command.trim();
    let mut cursor = 0usize;
    let mut segments = pattern.trim().split('*');
    let Some(first) = segments.next() else {
        return false;
    };
    if !command.starts_with(first) {
        return false;
    }
    cursor += first.len();
    let mut trailing_star = pattern.ends_with('*');
    for segment in segments {
        if segment.is_empty() {
            trailing_star = true;
            continue;
        }
        trailing_star = false;
        match command[cursor..].find(segment) {
            Some(offset) => cursor += offset + segment.len(),
            None => return false,
        }
    }
    trailing_star || cursor == command.len()
}

pub fn classify(rules: &AutoApprove, command: &str) -> Decision {
    if let Some(pattern) = rules.deny.iter().find(|pattern| matches(pattern, command)) {
        return Decision::Blocked(format!("command matches deny pattern `{pattern}`"));
    }
    if rules.allow.iter().any(|pattern| matches(pattern, command)) {
        return Decision::Allowed;
    }
    Decision::Ask
}

/// Per-broker rate limiting, duplicate suppression, and approval reuse.
///
/// ponytail: state is process-local and global across servers; move to a
/// per-alias map if one busy server ever starves the others.
#[derive(Default)]
pub struct Gate {
    executions: VecDeque<Instant>,
    recent: HashMap<String, Instant>,
    approvals: HashMap<String, Instant>,
}

impl Gate {
    /// Seconds to wait when the rolling hourly budget is spent.
    pub fn throttled_for(&mut self, now: Instant, limits: &Limits) -> Option<u64> {
        let hour = Duration::from_secs(3600);
        while self
            .executions
            .front()
            .is_some_and(|start| now.duration_since(*start) >= hour)
        {
            self.executions.pop_front();
        }
        if (self.executions.len() as u32) < limits.max_commands_per_hour {
            return None;
        }
        let oldest = self.executions.front()?;
        Some(
            hour.saturating_sub(now.duration_since(*oldest))
                .as_secs()
                .max(1),
        )
    }

    /// Seconds left in the dedup window for an identical command.
    pub fn duplicate_for(&self, hash: &str, now: Instant, limits: &Limits) -> Option<u64> {
        let window = Duration::from_secs(limits.dedup_seconds);
        if window.is_zero() {
            return None;
        }
        let last = self.recent.get(hash)?;
        let elapsed = now.duration_since(*last);
        (elapsed < window).then(|| window.saturating_sub(elapsed).as_secs().max(1))
    }

    pub fn approval_is_live(&mut self, hash: &str, now: Instant) -> bool {
        self.approvals.retain(|_, expiry| *expiry > now);
        self.approvals.contains_key(hash)
    }

    pub fn remember_approval(&mut self, hash: &str, now: Instant, limits: &Limits) {
        if limits.approval_ttl_seconds == 0 {
            return;
        }
        self.approvals.insert(
            hash.to_string(),
            now + Duration::from_secs(limits.approval_ttl_seconds),
        );
    }

    pub fn record_execution(&mut self, hash: &str, now: Instant) {
        self.executions.push_back(now);
        self.recent.insert(hash.to_string(), now);
        self.recent
            .retain(|_, seen| now.duration_since(*seen) < Duration::from_secs(3600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits::default()
    }

    #[test]
    fn glob_matches_only_intended_commands() {
        assert!(matches("docker logs *", "docker logs --tail 20 engine"));
        assert!(matches(
            "docker inspect -f *",
            "docker inspect -f '{{.Id}}' x"
        ));
        assert!(!matches("docker logs *", "docker rm engine"));
        assert!(matches("df -h", "df -h"));
        assert!(!matches("df -h", "df -h; rm -rf /"));
        assert!(matches("* > *", "cat a > b"));
        assert!(matches("curl *| *sh", "curl http://x | sudo sh"));
        assert!(!matches("docker ps*", "sudo docker ps"));
    }

    #[test]
    fn deny_beats_allow() {
        let rules = AutoApprove {
            allow: vec!["docker *".into()],
            deny: vec!["docker rm *".into()],
        };
        assert_eq!(
            classify(&rules, "docker rm engine"),
            Decision::Blocked("command matches deny pattern `docker rm *`".into())
        );
        assert_eq!(classify(&rules, "docker logs engine"), Decision::Allowed);
        assert_eq!(classify(&rules, "systemctl status app"), Decision::Ask);
    }

    #[test]
    fn hourly_budget_blocks_after_limit() {
        let mut gate = Gate::default();
        let mut limits = limits();
        limits.max_commands_per_hour = 2;
        let now = Instant::now();
        assert_eq!(gate.throttled_for(now, &limits), None);
        gate.record_execution("a", now);
        gate.record_execution("b", now);
        assert!(gate.throttled_for(now, &limits).is_some());
        let later = now + Duration::from_secs(3601);
        assert_eq!(gate.throttled_for(later, &limits), None);
    }

    #[test]
    fn identical_command_is_suppressed_inside_the_window() {
        let mut gate = Gate::default();
        let mut limits = limits();
        limits.dedup_seconds = 10;
        let now = Instant::now();
        gate.record_execution("hash", now);
        assert!(gate.duplicate_for("hash", now, &limits).is_some());
        assert!(
            gate.duplicate_for("hash", now + Duration::from_secs(11), &limits)
                .is_none()
        );
        assert!(gate.duplicate_for("other", now, &limits).is_none());
    }

    #[test]
    fn approval_expires_with_its_ttl() {
        let mut gate = Gate::default();
        let mut limits = limits();
        limits.approval_ttl_seconds = 60;
        let now = Instant::now();
        gate.remember_approval("hash", now, &limits);
        assert!(gate.approval_is_live("hash", now + Duration::from_secs(59)));
        assert!(!gate.approval_is_live("hash", now + Duration::from_secs(61)));
    }
}
