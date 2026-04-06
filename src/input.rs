use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    SldOnly,
    SldWithTldPrefix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedInput {
    pub original: String,
    pub normalized: String,
    pub sld: String,
    pub tld_prefix: Option<String>,
    pub mode: InputMode,
}

pub fn parse(input: &str) -> Result<ParsedInput, AppError> {
    if input.is_empty() {
        return Err(AppError::no_domain());
    }

    let lowered = input.to_ascii_lowercase();
    let normalized = if lowered.ends_with('.') {
        lowered[..lowered.len() - 1].to_string()
    } else {
        lowered.clone()
    };

    let dot_count = normalized.chars().filter(|ch| *ch == '.').count();
    if dot_count > 1 {
        return Err(
            AppError::new(format!("multi-label input '{normalized}' is not supported"))
                .with_where(format!("'{normalized}'"))
                .with_help("pass a single label like 'abc' or 'abc.co'"),
        );
    }

    if normalized.is_empty() {
        return Err(AppError::no_domain());
    }

    for (index, ch) in normalized.chars().enumerate() {
        if ch == '.' {
            continue;
        }

        if !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-') {
            return Err(AppError::new(format!(
                "invalid character '{ch}' in domain '{normalized}'"
            ))
            .with_where(format!("position {}", index + 1))
            .with_help("only letters (a-z), numbers (0-9), and hyphens (-) are allowed"));
        }
    }

    let (sld, tld_prefix, mode) = match normalized.split_once('.') {
        Some((left, right)) if !right.is_empty() => (
            left.to_string(),
            Some(right.to_string()),
            InputMode::SldWithTldPrefix,
        ),
        _ => (normalized.clone(), None, InputMode::SldOnly),
    };

    validate_label(&sld)?;

    if let Some(prefix) = &tld_prefix {
        validate_label(prefix)?;
    }

    Ok(ParsedInput {
        original: input.to_string(),
        normalized,
        sld,
        tld_prefix,
        mode,
    })
}

pub fn validate_label(label: &str) -> Result<(), AppError> {
    if label.is_empty() {
        return Err(AppError::no_domain());
    }

    if label.len() > 63 {
        return Err(AppError::new(format!(
            "domain too long ({} characters, max 63)",
            label.len()
        ))
        .with_where(format!("'{label}'"))
        .with_help("domain labels must be 63 characters or fewer (RFC 1035)"));
    }

    if label.starts_with('-') {
        return Err(AppError::new("domain cannot start with a hyphen")
            .with_where(format!("'{label}'"))
            .with_help("remove the leading hyphen, e.g., 'abc'"));
    }

    if label.ends_with('-') {
        return Err(AppError::new("domain cannot end with a hyphen")
            .with_where(format!("'{label}'"))
            .with_help("remove the trailing hyphen, e.g., 'abc'"));
    }

    let bytes = label.as_bytes();
    if bytes.len() >= 4 && bytes[2] == b'-' && bytes[3] == b'-' {
        return Err(
            AppError::new("domain cannot contain hyphens in positions 3 and 4")
                .with_where(format!("'{label}'"))
                .with_help("avoid the reserved '--' pattern used by punycode A-labels"),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse, validate_label, InputMode};

    #[test]
    fn parses_sld_only_input() {
        let parsed = parse("ABC.").unwrap();

        assert_eq!(parsed.mode, InputMode::SldOnly);
        assert_eq!(parsed.sld, "abc");
        assert_eq!(parsed.tld_prefix, None);
    }

    #[test]
    fn parses_sld_plus_prefix_input() {
        let parsed = parse("abc.Sh").unwrap();

        assert_eq!(parsed.mode, InputMode::SldWithTldPrefix);
        assert_eq!(parsed.sld, "abc");
        assert_eq!(parsed.tld_prefix.as_deref(), Some("sh"));
    }

    #[test]
    fn rejects_invalid_characters() {
        let err = parse("ab@c").unwrap_err().to_string();
        assert!(err.contains("invalid character '@'"));
        assert!(err.contains("position 3"));
    }

    #[test]
    fn rejects_reserved_hyphen_pattern() {
        let err = validate_label("ab--c").unwrap_err().to_string();
        assert!(err.contains("positions 3 and 4"));
    }

    #[test]
    fn rejects_multi_label_input() {
        let err = parse("abc.co.uk").unwrap_err().to_string();
        assert!(err.contains("multi-label input 'abc.co.uk' is not supported"));
    }

    #[test]
    fn trims_trailing_dot_before_counting_labels() {
        let parsed = parse("abc.co.").unwrap();
        assert_eq!(parsed.sld, "abc");
        assert_eq!(parsed.tld_prefix.as_deref(), Some("co"));
    }
}
