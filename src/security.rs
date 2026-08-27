use std::sync::OnceLock;

use regex::Regex;
use sha2::{Digest, Sha256};

pub fn redact(input: &[u8], exact_secrets: &[&str]) -> String {
    let mut output = String::from_utf8_lossy(input).into_owned();
    for secret in exact_secrets.iter().filter(|secret| !secret.is_empty()) {
        output = output.replace(secret, "[REDACTED]");
    }
    for pattern in patterns() {
        output = pattern.replace_all(&output, "$1[REDACTED]").into_owned();
    }
    output
}

/// How much of a growing buffer is safe to hand out before it is complete.
///
/// A secret can straddle a chunk boundary, so a partial last line is withheld,
/// and so is everything from an unterminated private-key header onwards.
pub fn releasable(text: &str, finished: bool) -> &str {
    let limit = if finished {
        text.len()
    } else {
        match text.rfind('\n') {
            Some(index) => index + 1,
            None => 0,
        }
    };
    let head = &text[..limit];
    match head.rfind("-----BEGIN") {
        Some(index) if !head[index..].contains("-----END") => &head[..index],
        _ => head,
    }
}

pub fn command_hash(command: &str) -> String {
    bytes_hash(command.as_bytes())
}

pub fn bytes_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)(authorization\s*:\s*bearer\s+)[A-Za-z0-9._~+/-]+=*",
            r#"(?i)([A-Za-z0-9_]*(?:password|passwd|token|api[_-]?key|secret|url|uri|dsn)\s*[=:]\s*)[^\s'\",]+"#,
            r"(?i)([a-z][a-z0-9+.-]*://[^\s:/]+:)[^@\s]+@",
            r"([^A-Za-z0-9]|^)AKIA[0-9A-Z]{16}",
            r"(?s)(-----BEGIN (?:OPENSSH |RSA |EC |DSA )?PRIVATE KEY-----).*?(-----END (?:OPENSSH |RSA |EC |DSA )?PRIVATE KEY-----)",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("constant regex must compile"))
        .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_exact_and_common_secrets() {
        let output = redact(
            b"pw=swordfish Authorization: Bearer abc.def token=xyz postgres://u:p@db",
            &["swordfish"],
        );
        assert!(!output.contains("swordfish"));
        assert!(!output.contains("abc.def"));
        assert!(!output.contains("xyz"));
        assert!(!output.contains(":p@"));
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn releasable_withholds_partial_lines_and_open_key_blocks() {
        assert_eq!(releasable("a\nb\npart", false), "a\nb\n");
        assert_eq!(releasable("a\nb\npart", true), "a\nb\npart");
        assert_eq!(releasable("log\n-----BEGIN KEY\nx\n", false), "log\n");
        assert_eq!(
            releasable("log\n-----BEGIN KEY\nx\n-----END KEY\n", false),
            "log\n-----BEGIN KEY\nx\n-----END KEY\n"
        );
    }

    #[test]
    fn redacts_container_env_and_cloud_keys() {
        let output = redact(
            b"-e DATABASE_URL=postgres://u:p@db/app AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE plain=keep",
            &[],
        );
        assert!(!output.contains("postgres://u:p@db/app"));
        assert!(!output.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(output.contains("plain=keep"));
    }
}
