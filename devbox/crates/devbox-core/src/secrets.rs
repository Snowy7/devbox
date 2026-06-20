//! Conservative local secret detection rules.

const MAX_SECRET_SCAN_BYTES: usize = 1024 * 1024;
const MAX_FINDINGS: usize = 16;

const AWS_ACCESS_KEY_ID: &str = "aws_access_key_id";
const GITHUB_TOKEN: &str = "github_token";
const OPENAI_API_KEY: &str = "openai_api_key";
const STRIPE_SECRET_KEY: &str = "stripe_secret_key";
const PRIVATE_KEY_PEM: &str = "private_key_pem";
const DOTENV_HIGH_ENTROPY_SECRET: &str = "dotenv_high_entropy_secret";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretFinding {
    rule_id: &'static str,
    line_number: usize,
    redacted_evidence: String,
}

impl SecretFinding {
    fn new(rule_id: &'static str, line_number: usize, redacted_evidence: String) -> Self {
        Self {
            rule_id,
            line_number,
            redacted_evidence,
        }
    }

    pub fn rule_id(&self) -> &'static str {
        self.rule_id
    }

    pub fn line_number(&self) -> usize {
        self.line_number
    }

    pub fn redacted_evidence(&self) -> &str {
        &self.redacted_evidence
    }

    pub fn policy_reason(&self) -> String {
        format!(
            "secret blocked by policy rule {} at line {}; evidence: {}",
            self.rule_id, self.line_number, self.redacted_evidence
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretDetector;

impl SecretDetector {
    pub fn scan_bytes(&self, bytes: &[u8]) -> Vec<SecretFinding> {
        let scan_bytes = &bytes[..bytes.len().min(MAX_SECRET_SCAN_BYTES)];
        if !looks_like_text(scan_bytes) {
            return Vec::new();
        }

        let text = String::from_utf8_lossy(scan_bytes);
        let mut findings = Vec::new();
        for (line_index, line) in text.lines().enumerate() {
            scan_line(line, line_index + 1, &mut findings);
            if findings.len() >= MAX_FINDINGS {
                findings.truncate(MAX_FINDINGS);
                break;
            }
        }

        findings
    }
}

fn scan_line(line: &str, line_number: usize, findings: &mut Vec<SecretFinding>) {
    if findings.len() >= MAX_FINDINGS {
        return;
    }

    let trimmed = line.trim();
    if is_private_key_header(trimmed) {
        findings.push(SecretFinding::new(
            PRIVATE_KEY_PEM,
            line_number,
            "-----BEGIN <redacted> PRIVATE KEY-----".to_string(),
        ));
        return;
    }

    scan_prefixed_token(
        trimmed,
        line_number,
        AWS_ACCESS_KEY_ID,
        "AKIA",
        16,
        "AKIA<redacted>",
        findings,
    );
    scan_prefixed_token(
        trimmed,
        line_number,
        AWS_ACCESS_KEY_ID,
        "ASIA",
        16,
        "ASIA<redacted>",
        findings,
    );
    scan_prefixed_token(
        trimmed,
        line_number,
        GITHUB_TOKEN,
        "ghp_",
        30,
        "ghp_<redacted>",
        findings,
    );
    scan_prefixed_token(
        trimmed,
        line_number,
        GITHUB_TOKEN,
        "github_pat_",
        30,
        "github_pat_<redacted>",
        findings,
    );
    scan_prefixed_token(
        trimmed,
        line_number,
        OPENAI_API_KEY,
        "sk-",
        32,
        "sk-<redacted>",
        findings,
    );
    scan_prefixed_token(
        trimmed,
        line_number,
        STRIPE_SECRET_KEY,
        "sk_live_",
        16,
        "sk_live_<redacted>",
        findings,
    );
    scan_prefixed_token(
        trimmed,
        line_number,
        STRIPE_SECRET_KEY,
        "sk_test_",
        16,
        "sk_test_<redacted>",
        findings,
    );

    if findings.len() < MAX_FINDINGS {
        if let Some(evidence) = dotenv_high_entropy_secret(trimmed) {
            findings.push(SecretFinding::new(
                DOTENV_HIGH_ENTROPY_SECRET,
                line_number,
                evidence,
            ));
        }
    }
}

fn scan_prefixed_token(
    line: &str,
    line_number: usize,
    rule_id: &'static str,
    prefix: &str,
    min_tail_len: usize,
    evidence: &str,
    findings: &mut Vec<SecretFinding>,
) {
    if findings.len() >= MAX_FINDINGS {
        return;
    }

    let mut search_start = 0;
    while let Some(offset) = line[search_start..].find(prefix) {
        let start = search_start + offset;
        let tail_start = start + prefix.len();
        let tail_len = line[tail_start..]
            .chars()
            .take_while(|character| is_token_char(*character))
            .count();
        let end = tail_start + tail_len;
        if tail_len >= min_tail_len && has_token_boundary(line, start, end) {
            findings.push(SecretFinding::new(
                rule_id,
                line_number,
                evidence.to_string(),
            ));
            return;
        }

        search_start = tail_start;
        if search_start >= line.len() {
            return;
        }
    }
}

fn dotenv_high_entropy_secret(line: &str) -> Option<String> {
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if !is_dotenv_secret_key(key) {
        return None;
    }

    let value = trim_value(value);
    if is_placeholder_value(value) || !looks_high_entropy(value) {
        return None;
    }

    Some(format!("{key}=<redacted>"))
}

fn trim_value(value: &str) -> &str {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .split(" #")
        .next()
        .unwrap_or("")
        .trim()
}

fn is_dotenv_secret_key(key: &str) -> bool {
    if key.is_empty()
        || !key.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return false;
    }

    [
        "SECRET",
        "TOKEN",
        "API_KEY",
        "ACCESS_KEY",
        "PRIVATE_KEY",
        "PASSWORD",
    ]
    .iter()
    .any(|marker| key.contains(marker))
}

fn looks_high_entropy(value: &str) -> bool {
    if value.len() < 24 || value.split_whitespace().count() > 1 {
        return false;
    }

    let has_lower = value
        .chars()
        .any(|character| character.is_ascii_lowercase());
    let has_upper = value
        .chars()
        .any(|character| character.is_ascii_uppercase());
    let has_digit = value.chars().any(|character| character.is_ascii_digit());
    let has_symbol = value
        .chars()
        .any(|character| matches!(character, '_' | '-' | '.' | '/' | '+' | '='));
    let classes = [has_lower, has_upper, has_digit, has_symbol]
        .into_iter()
        .filter(|present| *present)
        .count();

    classes >= 3
}

fn is_placeholder_value(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "example",
        "placeholder",
        "changeme",
        "change_me",
        "dummy",
        "redacted",
        "not-a-secret",
        "test",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn is_private_key_header(line: &str) -> bool {
    line.starts_with("-----BEGIN ")
        && line.ends_with(" PRIVATE KEY-----")
        && !line.contains("PUBLIC KEY")
}

fn is_token_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
}

fn has_token_boundary(line: &str, start: usize, end: usize) -> bool {
    let before_ok = line[..start]
        .chars()
        .next_back()
        .map_or(true, |character| !is_token_char(character));
    let after_ok = line[end..]
        .chars()
        .next()
        .map_or(true, |character| !is_token_char(character));
    before_ok && after_ok
}

fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }

    if bytes.contains(&0) {
        return false;
    }

    let control = bytes
        .iter()
        .filter(|byte| **byte < 0x20 && !matches!(**byte, b'\n' | b'\r' | b'\t'))
        .count();
    control * 100 / bytes.len() <= 5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_for(content: &str) -> Vec<&'static str> {
        SecretDetector
            .scan_bytes(content.as_bytes())
            .into_iter()
            .map(|finding| finding.rule_id())
            .collect()
    }

    fn token(prefix: &str, tail: &str) -> String {
        [prefix, tail].concat()
    }

    #[test]
    fn detects_high_confidence_provider_tokens() {
        let aws = token("AKIA", "IOSFODNN7EXAMPLE1");
        let github = token("ghp_", "abcdefghijklmnopqrstuvwxyz1234567890");
        let github_fine = token("github_pat_", "11AAabcdefghijklmnopqrstuvwxyz1234567890");
        let openai = token("sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456");
        let stripe = token("sk_live_", "abcdefghijklmnopqrstuvwxyz12");
        let content = [
            format!("aws = \"{aws}\""),
            format!("github = \"{github}\""),
            format!("github_fine = \"{github_fine}\""),
            format!("openai = \"{openai}\""),
            format!("stripe = \"{stripe}\""),
        ]
        .join("\n");

        let rules = rules_for(&content);

        assert!(rules.contains(&AWS_ACCESS_KEY_ID));
        assert!(rules.contains(&GITHUB_TOKEN));
        assert!(rules.contains(&OPENAI_API_KEY));
        assert!(rules.contains(&STRIPE_SECRET_KEY));
    }

    #[test]
    fn detects_private_key_pem_header() {
        assert_eq!(
            rules_for("-----BEGIN OPENSSH PRIVATE KEY-----\nabc"),
            vec![PRIVATE_KEY_PEM]
        );
    }

    #[test]
    fn detects_dotenv_style_high_entropy_secret_assignments() {
        let value = ["AbCdEfGhIjKlMnOp", "QrStUvWxYz123456"].concat();
        let content = format!("SESSION_SECRET={value}\n");
        let findings = SecretDetector
            .scan_bytes(content.as_bytes())
            .into_iter()
            .collect::<Vec<_>>();

        assert_eq!(findings[0].rule_id(), DOTENV_HIGH_ENTROPY_SECRET);
        assert_eq!(findings[0].line_number(), 1);
        assert_eq!(findings[0].redacted_evidence(), "SESSION_SECRET=<redacted>");
    }

    #[test]
    fn ignores_benign_near_misses_and_examples() {
        let content = [
            "let token = \"ghp_example_placeholder\";".to_string(),
            "OPENAI_API_KEY=sk-example".to_string(),
            "SESSION_SECRET=changeme".to_string(),
            "password = \"correct horse battery staple\"".to_string(),
            format!("AWS_ACCESS_KEY_ID={}", token("AKIA", "IOSFODNN7EXAMPL")),
            "-----BEGIN PUBLIC KEY-----".to_string(),
        ]
        .join("\n");

        assert!(SecretDetector.scan_bytes(content.as_bytes()).is_empty());
    }

    #[test]
    fn scans_only_bounded_text_prefix_and_skips_binary() {
        let mut large = vec![b'a'; MAX_SECRET_SCAN_BYTES + 128];
        let openai = token("sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456");
        let start = MAX_SECRET_SCAN_BYTES - 64;
        large[start..start + openai.len()].copy_from_slice(openai.as_bytes());
        large[start - 1] = b'\n';
        large[start + openai.len()] = b'\n';
        assert_eq!(SecretDetector.scan_bytes(&large).len(), 1);

        let binary = [b"abc\0".as_slice(), openai.as_bytes()].concat();
        assert!(SecretDetector.scan_bytes(&binary).is_empty());
    }

    #[test]
    fn policy_reason_never_contains_raw_secret_value() {
        let raw = token("sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456");
        let finding = SecretDetector
            .scan_bytes(raw.as_bytes())
            .into_iter()
            .next()
            .expect("secret is detected");

        assert!(!finding.policy_reason().contains(&raw));
        assert!(finding.policy_reason().contains("sk-<redacted>"));
    }
}
