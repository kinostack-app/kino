//! Secret scrubbing for log text.
//!
//! First line of defence is the `secrecy::SecretString` newtype wrapping
//! our credential config fields — those never produce plaintext in
//! `Debug`/`Display`. This module is the belt-and-braces regex sweep
//! applied to every log record just before it's serialised for the
//! writer: it catches cases where a third-party dependency (librqbit,
//! reqwest redirect handling, a tracker's error response, …) embeds a
//! secret in its own output.
//!
//! Rules of thumb:
//!   - False positives are cheap (a redacted string you wanted to see is
//!     curiosity).
//!   - False negatives are expensive (a leaked passkey is a banned
//!     tracker account).
//!
//! Errs toward aggressive.

use std::sync::LazyLock;

use regex::Regex;

struct Rule {
    re: Regex,
    replacement: &'static str,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        // Magnet URIs — replace the whole URI including trackers / dn /
        // passkeys. Keeps the hash so logs are still useful.
        Rule {
            re: Regex::new(r"magnet:\?xt=urn:btih:([0-9a-fA-F]{40})[^\s\x22]*")
                .expect("valid regex"),
            replacement: "magnet:?xt=urn:btih:$1&[REDACTED]",
        },
        // Tracker passkeys in URLs.
        Rule {
            re: Regex::new(r"passkey=[A-Za-z0-9]{16,}").expect("valid regex"),
            replacement: "passkey=[REDACTED]",
        },
        Rule {
            re: Regex::new(r"announce/[A-Za-z0-9]{16,}").expect("valid regex"),
            replacement: "announce/[REDACTED]",
        },
        // `api_key=…` / `apikey=…` query params or field values.
        Rule {
            re: Regex::new(r"(?i)\bapi_?key=[A-Za-z0-9\-]+").expect("valid regex"),
            replacement: "api_key=[REDACTED]",
        },
        // Authorization: Bearer …
        Rule {
            re: Regex::new(r"(?i)Authorization:\s*Bearer\s+\S+").expect("valid regex"),
            replacement: "Authorization: Bearer [REDACTED]",
        },
        Rule {
            re: Regex::new(r#"["']?Authorization["']?\s*:\s*["']Bearer\s+[^"']+["']"#)
                .expect("valid regex"),
            replacement: r#""Authorization":"Bearer [REDACTED]""#,
        },
        // Bare bearer tokens in body JSON — `"token":"..."` and `"access_token":"..."`.
        Rule {
            re: Regex::new(r#""(?:access_?token|refresh_?token|token)"\s*:\s*"[^"]+""#)
                .expect("valid regex"),
            replacement: r#""token":"[REDACTED]""#,
        },
        // WireGuard-style 32-byte base64 keys (exactly 43 chars + `=`).
        Rule {
            re: Regex::new(r"\b[A-Za-z0-9+/]{43}=").expect("valid regex"),
            replacement: "[REDACTED_WG_KEY]",
        },
        // OpenSubtitles password in login JSON.
        Rule {
            re: Regex::new(r#""password"\s*:\s*"[^"]+""#).expect("valid regex"),
            replacement: r#""password":"[REDACTED]""#,
        },
    ]
});

/// Scrub a string in place (returning the redacted version). Cheap when
/// there's nothing to match — the regex set is walked by reference.
#[must_use]
pub fn redact(input: &str) -> String {
    let mut out = input.to_owned();
    for rule in RULES.iter() {
        if rule.re.is_match(&out) {
            out = rule.re.replace_all(&out, rule.replacement).into_owned();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_magnet_passkey() {
        let input = "magnet:?xt=urn:btih:1234567890abcdef1234567890abcdef12345678&tr=http://t/announce/abcdef1234567890abcdef1234567890";
        let out = redact(input);
        assert!(out.contains("1234567890abcdef1234567890abcdef12345678"));
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("abcdef1234567890abcdef1234567890"));
    }

    #[test]
    fn redacts_bearer() {
        let input = "Authorization: Bearer sk-abc123xyz";
        assert_eq!(redact(input), "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redacts_api_key_query() {
        let input = "https://example.com/?t=caps&apikey=abc123";
        let out = redact(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("abc123"));
    }

    #[test]
    fn redacts_wg_private_key() {
        // 43 base64 chars + `=`
        let key = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq=";
        assert_eq!(key.len(), 44);
        let input = format!("wg config: private_key = {key}");
        let out = redact(&input);
        assert!(out.contains("[REDACTED_WG_KEY]"));
        assert!(!out.contains(key));
    }

    #[test]
    fn redacts_json_password() {
        let input = r#"{"username":"me","password":"hunter2"}"#;
        let out = redact(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("hunter2"));
    }

    #[test]
    fn leaves_ordinary_strings_untouched() {
        let input = "download completed — 1234 MB in 45s";
        assert_eq!(redact(input), input);
    }
}
