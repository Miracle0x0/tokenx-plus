use anyhow::Result;

use super::{SubscriptionIssue, SubscriptionIssueCode};

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s.to_string(),
    }
}

pub fn read_keychain(service: &str) -> Result<Option<String>> {
    if cfg!(not(target_os = "macos")) {
        return Err(anyhow::Error::new(SubscriptionIssue::new(
            SubscriptionIssueCode::KeychainMacOnly,
            "Keychain lookup is only available on macOS",
        )));
    }
    let out = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()?;
    decode_keychain_output(service, out.status.code(), out.stdout, &out.stderr)
}

fn decode_keychain_output(
    service: &str,
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: &[u8],
) -> Result<Option<String>> {
    // SecurityTool returns errSecItemNotFound (-25300, exit status 44). It can
    // also return 44 after a different search failure, printing both errors:
    // only the sole, exact not-found diagnostic establishes absence.
    // https://github.com/apple-oss-distributions/Security/blob/main/SecurityTool/macOS/keychain_find.c
    if code == Some(44) && stderr == b"security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain.\n" {
        return Ok(None);
    }
    if code != Some(0) {
        return Err(anyhow::Error::new(
            SubscriptionIssue::new(
                SubscriptionIssueCode::KeychainLookupFailed,
                format!("Keychain lookup failed for service '{service}'"),
            )
            .with_field("service", service),
        ));
    }
    Ok(Some(String::from_utf8(stdout)?.trim_end().to_string()))
}

pub fn read_env(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod keychain_tests {
    use super::*;

    #[test]
    fn only_an_unambiguous_missing_item_is_absence() {
        let missing = b"security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain.\n";
        assert!(
            decode_keychain_output("test", Some(44), Vec::new(), missing)
                .unwrap()
                .is_none()
        );
        for (code, stderr) in [
            (Some(44), b"access denied\n".as_slice()),
            (Some(1), missing.as_slice()),
            (None, b"".as_slice()),
        ] {
            assert!(decode_keychain_output("test", code, Vec::new(), stderr).is_err());
        }
        let multiple = [b"security: search failed\n".as_slice(), missing].concat();
        assert!(decode_keychain_output("test", Some(44), Vec::new(), &multiple).is_err());
        assert_eq!(
            decode_keychain_output("test", Some(0), b"synthetic\n".to_vec(), b"")
                .unwrap()
                .as_deref(),
            Some("synthetic")
        );
    }
}
