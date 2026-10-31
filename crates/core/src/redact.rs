//! Wrapper for values that must never reach logs or error messages.

use std::fmt;

/// Holds a secret, such as a fragment URL that embeds the client IP.
///
/// `Debug` and `Display` print a placeholder. Reading the value takes an explicit `expose` call.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wraps a secret value.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Returns the secret. Callers must not log the result.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_hides_the_value() {
        let secret = Redacted::new("https://host/?ip=192.0.2.1".to_string());

        assert_eq!(format!("{secret}"), "[redacted]");
        assert_eq!(format!("{secret:?}"), "[redacted]");
    }

    #[test]
    fn expose_returns_the_value() {
        assert_eq!(Redacted::new(7).expose(), &7);
    }
}
