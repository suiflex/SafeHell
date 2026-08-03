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

pub fn command_hash(command: &str) -> String {
    let digest = Sha256::digest(command.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)(authorization\s*:\s*bearer\s+)[A-Za-z0-9._~+/-]+=*",
            r#"(?i)((?:password|passwd|token|api[_-]?key|secret)\s*[=:]\s*)[^\s'\"]+"#,
            r"(?i)([a-z][a-z0-9+.-]*://[^\s:/]+:)[^@\s]+@",
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
}
