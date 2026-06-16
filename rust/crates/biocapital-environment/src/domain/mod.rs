//! Environment-effect domain types — `doc/07-environment.md` §2 + §3 + §4 + §5.
//!
//! Public surface (re-exported by [`crate::domain`]):
//! - [`environment::EnvironmentType`] — 4 canonical values +
//!   `Fluid(String)` escape hatch for the FLUID_<X> tokens.
//! - [`environment::EnvironmentModifier`] — 6-value enum covering
//!   the pleasure / hunger / movement / flag mutations.
//! - [`environment::IntensityFormula`] — 3-value enum covering
//!   fixed / linear-distance / fixed-duration rules.
//! - [`environment::EnvironmentSource`] — 3-value enum (block
//!   contact / fluid immersion / air exposure).
//! - [`environment::EnvironmentEffectRule`] — the in-memory shape
//!   for a single rule row.
//! - [`environment::DEFAULT_ENVIRONMENT_RULES`] — the in-process
//!   seed for the 4 canonical environments (mirrors the
//!   `environment_default_rules` table seed in
//!   `rust/migrations/20260614000008_environment.sql`).
//! - [`environment::EnvironmentParseError`] /
//!   [`environment::EnvironmentSourceParseError`] — `FromStr`
//!   errors.

pub mod environment;

pub use environment::{
    EnvironmentEffectRule, EnvironmentModifier, EnvironmentParseError, EnvironmentSource,
    EnvironmentSourceParseError, EnvironmentType, IntensityFormula, DEFAULT_ENVIRONMENT_RULES,
};
