//! SSH private keys as people paste them: line endings fixed up, then checked the same way by the
//! clients before sending and by the daemon before storing.

use ssh_key::{HashAlg, PrivateKey, public::KeyData};

/// What a valid private key is, for showing before it is saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInfo {
    /// `ED25519`, `RSA 3072`, `ECDSA nistp256`, close to how `ssh-keygen -l` names them.
    pub kind: String,
    /// `SHA256:…`
    pub fingerprint: String,
    pub comment: String,
    /// The OpenSSH public key line.
    pub public_key: String,
}

impl KeyInfo {
    /// `ED25519 · SHA256:… · comment`
    pub fn summary(&self) -> String {
        let mut s = format!("{} · {}", self.kind, self.fingerprint);
        if !self.comment.is_empty() {
            s.push_str(" · ");
            s.push_str(&self.comment);
        }
        s
    }
}

/// Turns CRLF and lone CR into LF. Terminals send a pasted line break as CR, the Enter key.
pub fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// A key as OpenSSH reads it: LF line endings, no blank or padded lines, one trailing newline.
/// OpenSSH refuses a key file with CR line endings or without the final newline.
pub fn normalize_private_key(text: &str) -> String {
    let mut key = String::with_capacity(text.len() + 1);
    for line in normalize_newlines(text).lines().map(str::trim) {
        if !line.is_empty() {
            key.push_str(line);
            key.push('\n');
        }
    }
    key
}

/// Checks a private key and describes it. Errors say what is wrong without echoing the key.
pub fn inspect_private_key(text: &str) -> Result<KeyInfo, String> {
    let key = normalize_private_key(text);
    let first = key.lines().next().unwrap_or_default();
    if first.is_empty() {
        return Err("paste a private key".into());
    }
    if ["ssh-", "ecdsa-", "sk-"]
        .iter()
        .any(|p| first.starts_with(p))
        || first.contains("PUBLIC KEY")
    {
        return Err("this looks like a public key: paste the private key, not the .pub".into());
    }
    let Some(label) = first
        .strip_prefix("-----BEGIN ")
        .and_then(|l| l.strip_suffix("-----"))
    else {
        return Err(
            "not an OpenSSH private key: expected a -----BEGIN OPENSSH PRIVATE KEY----- block"
                .into(),
        );
    };
    let end = format!("-----END {label}-----");
    if !key.lines().any(|l| l == end) {
        return Err(format!("truncated key: missing the {end} line"));
    }
    if label == "ENCRYPTED PRIVATE KEY" || key.contains("Proc-Type: 4,ENCRYPTED") {
        return Err(ENCRYPTED.into());
    }
    if label != "OPENSSH PRIVATE KEY" {
        return Err(format!(
            "{label} is the old PEM format: convert it with `ssh-keygen -p -f <file>`"
        ));
    }
    let parsed = PrivateKey::from_openssh(&key)
        .map_err(|e| format!("damaged key: the lines between BEGIN and END do not decode ({e})"))?;
    if parsed.is_encrypted() {
        return Err(ENCRYPTED.into());
    }
    let public = parsed.public_key();
    Ok(KeyInfo {
        kind: match public.key_data() {
            KeyData::Ed25519(_) => "ED25519".into(),
            KeyData::Rsa(rsa) => match rsa.n.as_positive_bytes() {
                Some(n) => format!("RSA {}", n.len() * 8),
                None => "RSA".into(),
            },
            KeyData::Ecdsa(ecdsa) => format!("ECDSA {}", ecdsa.curve()),
            other => other.algorithm().as_str().to_uppercase(),
        },
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
        comment: parsed.comment().to_string(),
        public_key: public
            .to_openssh()
            .map_err(|e| format!("cannot encode the public key: {e}"))?,
    })
}

const ENCRYPTED: &str =
    "encrypted key: passphrases are not supported; remove it with `ssh-keygen -p -f <file>`";

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway key made with `ssh-keygen -t ed25519 -C test@host` for these tests only.
    const ED25519: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDZ4o0ESt/JCpKOiZiDaKKOV16n0HI12f0dGZMlWAEXeAAAAJCor356qK9+
egAAAAtzc2gtZWQyNTUxOQAAACDZ4o0ESt/JCpKOiZiDaKKOV16n0HI12f0dGZMlWAEXeA
AAAEDda5c286pWIXEajsx5q6poMu1Ef3rk/eL3fa9JEKdpENnijQRK38kKko6JmINooo5X
XqfQcjXZ/R0ZkyVYARd4AAAACXRlc3RAaG9zdAECAwQ=
-----END OPENSSH PRIVATE KEY-----
";

    #[test]
    fn terminal_line_endings_become_a_key_openssh_reads() {
        for pasted in [
            ED25519.replace('\n', "\r"),
            ED25519.replace('\n', "\r\n"),
            format!("  {}  ", ED25519.trim_end()),
            ED25519.replace('\n', "\n\n"),
        ] {
            assert_eq!(normalize_private_key(&pasted), ED25519);
        }
    }

    #[test]
    fn a_valid_key_is_described() {
        let info = inspect_private_key(&ED25519.replace('\n', "\r")).unwrap();
        assert_eq!(info.kind, "ED25519");
        assert!(info.fingerprint.starts_with("SHA256:"), "{info:?}");
        assert_eq!(info.comment, "test@host");
        assert!(info.public_key.starts_with("ssh-ed25519 AAAA"));
        assert_eq!(
            info.summary(),
            format!("ED25519 · {} · test@host", info.fingerprint)
        );
    }

    #[test]
    fn broken_keys_get_a_specific_error() {
        let err = |text: &str| inspect_private_key(text).unwrap_err();
        assert_eq!(err("  \n"), "paste a private key");
        assert!(err("ssh-ed25519 AAAAC3Nza test@host").contains("public key"));
        assert!(err("---- BEGIN SSH2 PUBLIC KEY ----").contains("public key"));
        let cut: String = ED25519.lines().take(3).collect::<Vec<_>>().join("\n");
        assert_eq!(
            err(&cut),
            "truncated key: missing the -----END OPENSSH PRIVATE KEY----- line"
        );
        assert!(
            err("-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----")
                .contains("PEM")
        );
        assert!(
            err("-----BEGIN ENCRYPTED PRIVATE KEY-----\nabc\n-----END ENCRYPTED PRIVATE KEY-----")
                .starts_with("encrypted key")
        );
        let mut lines: Vec<&str> = ED25519.lines().collect();
        lines.remove(2);
        assert!(err(&lines.join("\n")).starts_with("damaged key"));
        assert!(err("hunter2").starts_with("not an OpenSSH private key"));
        assert!(!err("hunter2").contains("hunter2"));
    }
}
