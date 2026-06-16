//! `biocapital-pg` — PostgreSQL schema, migrations, and repositories.
//!
//! See `doc/14-rust-services.md` for the overall architecture and
//! `doc/99-integration-matrix.md` §5 for the table-to-module mapping.
//!
//! Module organisation (post task #134 cycle-fix):
//! - **Cross-domain repos** stay in this crate because the
//!   corresponding domain crates (bank, contract, core_pod, dglab,
//!   pod) do NOT depend on `biocapital-pg` — moving them would not
//!   help the cycle and would split the workspace along an
//!   unhelpful seam:
//!   - `bank` / `contract` / `core_pod` / `dglab` / `mob_replacement`
//!     are persistence-only companions of their respective domain
//!     types.
//! - **Player-state repos** stay here because the
//!   `PlayerStateService` is the workspace's central read/write
//!   path; almost every gRPC service depends on
//!   `PlayerStateServiceDeps`. Splitting it would force every
//!   other crate to declare a transitive dep on `biocapital-core`
//!   just to build the deps handle.
//! - **Per-domain repos that *did* form cycles** (environment +
//!   fluid + creature) were moved into their owning domain crate
//!   under `pg::` (see `biocapital-environment::pg` and
//!   `biocapital-creature::pg`).

pub mod bank;
pub mod contract;
pub mod core_pod;
pub mod dglab;
pub mod hardware_token;
pub mod player_state;

pub use bank::{
    BankAuditEntry, BankAuditWriter, BankRepository, BankServiceDeps, BankRepoError,
    PgBankAuditWriter, PgBankRepository,
};
pub use contract::{
    BankTransferPort, ContractAuditEntry, ContractAuditWriter, ContractRepository,
    ContractServiceDeps, ContractRepoError, PgContractAuditWriter, PgContractRepository,
};
pub use core_pod::{
    CorePodAuditEntry, CorePodAuditWriter, CorePodRepository, CorePodServiceDeps,
    PgCorePodAuditWriter, PgCorePodRepository, PodIdentifier, PodRepoError,
};
pub use dglab::{
    DglabAuditEntry, DglabAuditWriter, DglabOverride, DglabOverrideParam, DglabOverrideRepository,
    DglabRepository, DglabServiceDeps, DglabRepoError, DglabStrengthLog,
    PgDglabAuditWriter, PgDglabRepository, PlayerDglabConfig, PlayerDglabConfigRepository,
};
pub use hardware_token::{
    HardwareTokenAuditEntry, HardwareTokenAuditWriter, HardwareTokenRepository,
    HardwareTokenRepoError, HardwareTokenServiceDeps, PgHardwareTokenAuditWriter,
    PgHardwareTokenRepository,
};
pub use player_state::{
    AuditEntry, AuditWriter, PgAuditWriter, PgPlayerStateLoader, PgPlayerStateRepository,
    PlayerStateRepository, PlayerStateServiceDeps, RepoError as PlayerStateRepoError,
};

/// Placeholder kept for callers that may still depend on a no-op symbol.
/// New code should use the typed re-exports above.
pub fn placeholder() {}
