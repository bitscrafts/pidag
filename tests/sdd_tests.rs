//! Tests for the SDD (Spec-Driven Development) module.

use pidag::{SpecError, validate_spec_name};

#[test]
fn test_spec_name_valid() {
    assert!(validate_spec_name("01-fibonacci.md").is_ok());
    assert!(validate_spec_name("02-binary-search.md").is_ok());
    assert!(validate_spec_name("13-my-feature.md").is_ok());
    assert!(validate_spec_name("99-zzz.md").is_ok());
}

#[test]
fn test_spec_name_invalid_no_prefix() {
    let res = validate_spec_name("fibonacci.md");
    assert!(matches!(res, Err(SpecError::InvalidName(_))));
}

#[test]
fn test_spec_name_invalid_bad_prefix() {
    // Single digit prefix (not exactly NN)
    assert!(matches!(
        validate_spec_name("1-fibonacci.md"),
        Err(SpecError::InvalidName(_))
    ));
    // Non-numeric prefix
    assert!(matches!(
        validate_spec_name("ab-fibonacci.md"),
        Err(SpecError::InvalidName(_))
    ));
    // Three-digit prefix
    assert!(matches!(
        validate_spec_name("123-fibonacci.md"),
        Err(SpecError::InvalidName(_))
    ));
    // Zero prefix outside 01..=99
    assert!(matches!(
        validate_spec_name("00-fibonacci.md"),
        Err(SpecError::InvalidName(_))
    ));
}

#[test]
fn test_spec_name_invalid_misc() {
    // Missing .md extension
    assert!(matches!(
        validate_spec_name("01-fibonacci"),
        Err(SpecError::InvalidName(_))
    ));
    // No hyphen
    assert!(matches!(
        validate_spec_name("01fibonacci.md"),
        Err(SpecError::InvalidName(_))
    ));
    // Path traversal
    assert!(matches!(
        validate_spec_name("../01-fibonacci.md"),
        Err(SpecError::InvalidName(_))
    ));
    // Uppercase slug
    assert!(matches!(
        validate_spec_name("01-Fibonacci.md"),
        Err(SpecError::InvalidName(_))
    ));
}
