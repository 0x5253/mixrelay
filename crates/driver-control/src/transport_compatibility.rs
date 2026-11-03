//! Service configuration for exact bundle identifiers, never authorization.
use crate::DriverError;

/// Parses the development environment override; empty text disables compatibility.
/// The service supplies its default when the environment variable is absent.
/// # Errors
/// Rejects more than 32 entries, duplicates, empty entries and identifiers of
/// 128 bytes or more. Identifiers use ASCII letters, digits, hyphens and dots.
pub fn parse_transport_compatibility(value: &str) -> Result<Vec<String>, DriverError> {
    let bundles: Vec<String> = if value.is_empty() {
        Vec::new()
    } else {
        value.split(',').map(str::to_owned).collect()
    };
    if bundles.len() > 32
        || bundles.iter().enumerate().any(|(index, bundle)| {
            bundle.is_empty()
                || bundle.len() >= 128
                || bundles[..index].contains(bundle)
                || !bundle.split('.').all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                })
        })
    {
        return Err(DriverError::Invalid(
            "invalid transport compatibility bundle list".into(),
        ));
    }
    Ok(bundles)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_is_exact_bounded_and_can_be_disabled() {
        assert!(parse_transport_compatibility("").unwrap().is_empty());
        assert_eq!(
            parse_transport_compatibility("com.example.A,net.example.B").unwrap(),
            ["com.example.A", "net.example.B"]
        );
        for invalid in [
            "com..app",
            ".app",
            "app.",
            "com.app,",
            "com.app,com.app",
            "com.app*",
            "com.app\0",
            " com.app",
            "é.app",
        ] {
            assert!(
                parse_transport_compatibility(invalid).is_err(),
                "{invalid:?}"
            );
        }
        assert!(parse_transport_compatibility(&"a".repeat(128)).is_err());
        let list = (0..32)
            .map(|index| format!("com.app{index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            parse_transport_compatibility(&list.join(","))
                .unwrap()
                .len(),
            32
        );
        assert!(parse_transport_compatibility(&format!("{},com.extra", list.join(","))).is_err());
    }
}
