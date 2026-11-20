//! Sensitive form field detection and redaction for browser accessibility snapshots
//! (`packages/gui-plugins/src/browser/field-value.ts`).
//!
//! Fail-closed: if a field's metadata cannot be read or parsed, [`readable_field`] is never called
//! with synthetic defaults and callers print `<hidden>`.

use serde_json::Value;

/// CDP `Runtime.callFunctionOn` body that inspects one form element's type, autocomplete,
/// labels, CSS text-masking (`-webkit-text-security`), `maxLength`, and adjacent toggle buttons.
/// Buttons count as the field's own only when it is the sole field in its parent, so a login
/// form's "Show password" button does not hide its username field (`field-value.ts:18-32`).
pub const DESCRIBE_FIELD_JS: &str = r#"function() {
  const parent = this.parentElement;
  const own = parent && parent.querySelectorAll("input, textarea, [contenteditable]").length === 1;
  return {
    type: this instanceof HTMLInputElement ? this.type : "",
    autocomplete: this.getAttribute("autocomplete") ?? "",
    labels: [
      this.id,
      this.getAttribute("name"),
      this.getAttribute("placeholder"),
      this.getAttribute("aria-label"),
      ...Array.from(this.labels ?? [], (label) => label.textContent)
    ].filter(Boolean),
    masked: getComputedStyle(this).webkitTextSecurity !== "none",
    maxLength: this instanceof HTMLInputElement ? this.maxLength : -1,
    toggles: own
      ? Array.from(parent.querySelectorAll('button, [role="button"]'), (button) => button.getAttribute("aria-label") || button.textContent).filter(Boolean)
      : []
  };
}"#;

/// Decoded metadata for one form control (`field-value.ts:4-15`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    pub field_type: String,
    pub autocomplete: String,
    pub labels: Vec<String>,
    pub masked: bool,
    pub max_length: i64,
    pub toggles: Vec<String>,
}

impl FieldInfo {
    pub fn from_value(v: &Value) -> Option<Self> {
        let obj = v.as_object()?;
        let field_type = obj.get("type")?.as_str()?.to_owned();
        let autocomplete = obj.get("autocomplete")?.as_str()?.to_owned();
        let masked = obj.get("masked")?.as_bool()?;
        let max_length = obj.get("maxLength")?.as_i64()?;
        let labels = obj
            .get("labels")?
            .as_array()?
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect();
        let toggles = obj
            .get("toggles")?
            .as_array()?
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect();
        Some(Self { field_type, autocomplete, labels, masked, max_length, toggles })
    }
}

/// Whether a snapshot may print `field`'s value (`field-value.ts:44-60`).
/// Hides passwords (even after a "show password" toggle flips `type="text"`), file inputs,
/// `-webkit-text-security` masked inputs, 1-char OTP/PIN cells, secret `autocomplete` tokens,
/// and fields whose accessible name, labels, or own toggle buttons contain secret keywords.
pub fn readable_field(field: &FieldInfo, accessible_name: &str) -> bool {
    if field.masked || matches!(field.field_type.as_str(), "password" | "file") {
        return false;
    }
    if field.max_length == 1 && matches!(field.field_type.as_str(), "text" | "tel" | "number") {
        return false;
    }
    if field.autocomplete.split_whitespace().any(|tok| {
        let lower = tok.to_ascii_lowercase();
        matches!(lower.as_str(), "current-password" | "new-password" | "one-time-code")
            || lower.starts_with("cc-")
    }) {
        return false;
    }
    let all = std::iter::once(accessible_name)
        .chain(field.labels.iter().map(String::as_str))
        .chain(field.toggles.iter().map(String::as_str));
    for raw in all {
        if has_secret_word(&normalize_label(raw)) {
            return false;
        }
    }
    true
}

/// Splits camelCase/PascalCase runs, replaces non-word chars/underscores with spaces, and lowercases
/// (`field-value.ts:53-57`).
fn normalize_label(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut split = String::with_capacity(raw.len() + 8);
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            let camel_edge = prev.is_ascii_lowercase() && ch.is_ascii_uppercase();
            let acronym_edge =
                prev.is_ascii_uppercase() && ch.is_ascii_uppercase() && next.is_some_and(|n| n.is_ascii_lowercase());
            if camel_edge || acronym_edge {
                split.push(' ');
            }
        }
        if ch.is_ascii_alphanumeric() {
            split.push(ch.to_ascii_lowercase());
        } else {
            split.push(' ');
        }
    }
    split
}

fn has_secret_word(normalized: &str) -> bool {
    if normalized.contains("passw")
        || normalized.contains("passcode")
        || normalized.contains("secret")
    {
        return true;
    }
    let words: Vec<&str> = normalized.split_whitespace().collect();
    for (i, &w) in words.iter().enumerate() {
        if matches!(w, "token" | "pwd" | "otp" | "totp" | "2fa" | "mfa" | "pin" | "cvv" | "cvc" | "ssn") {
            return true;
        }
        if matches!(
            w,
            "apikey"
                | "privatekey"
                | "accesskey"
                | "signingkey"
                | "authkey"
                | "sshkey"
                | "webhookkey"
                | "encryptionkey"
                | "securitycode"
                | "verificationcode"
                | "recoverycode"
                | "backupcode"
                | "cardnumber"
                | "cardnum"
                | "cardno"
        ) {
            return true;
        }
        if let Some(&next) = words.get(i + 1) {
            if next == "key"
                && matches!(
                    w,
                    "api" | "private" | "access" | "signing" | "auth" | "ssh" | "webhook" | "encryption"
                )
            {
                return true;
            }
            if next == "code"
                && matches!(w, "security" | "verification" | "recovery" | "backup")
            {
                return true;
            }
            if w.ends_with("card") && matches!(next, "number" | "num" | "no") {
                return true;
            }
        }
        if w.ends_with("cardnumber") || w.ends_with("cardnum") || w.ends_with("cardno") {
            return true;
        }
    }
    false
}
