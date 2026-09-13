//! The bridge's wire API, as a version.
//!
//! One number for the whole wire: what `session.hello` and `ping` report, and
//! what a client's declared `api_range` is measured against. Semver, per the
//! wire spec (Part 2): a patch changes nothing on the wire, a minor only adds,
//! a major is the only thing that removes or reshapes.

pub mod clients;

/// The version of the wire API this bridge speaks.
pub const API_VERSION: &str = "1.0.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_version_is_three_dot_separated_integers() {
        let parts: Vec<&str> = API_VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "{API_VERSION}");
        for part in parts {
            assert!(
                part.parse::<u64>().is_ok(),
                "{part:?} in {API_VERSION} is not an integer"
            );
        }
    }
}
