//! `biocapital-grpc` — gRPC server + client (tonic) per `doc/14-rust-services.md`.
//!
//! Per-crate services are re-exported from this top-level `lib.rs`. The
//! `build.rs` that wires `prost-build` is part of task #17 follow-up; for
//! now the per-service modules define proto-shaped Rust structs and a
//! gRPC-trait abstraction so the rest of the system compiles.

pub mod bank_service;
pub mod contract_service;
pub mod core_pod_service;
pub mod creature_service;
pub mod dglab_service;
pub mod environment_service;
pub mod hostile_mob_service;
pub mod player_state_service;

pub use bank_service::{
    AcceptInviteRequest, AccountRequest, AuthenticateRequest, AuthenticateResponse,
    BalanceResponse, BankGrpc, BankRpc, BindHardwareRequest, DepositRequest,
    EventMeta as BankEventMeta, HistoryRequest, HistoryResponse, InviteCodeResponse,
    LockDeviceRequest, LockDeviceResponse, PlayerIdentifier as BankPlayerIdentifier,
    RevokeHardwareRequest, TransferRequest, UnlockDeviceRequest, WithdrawRequest,
};
pub use contract_service::{
    ContractAcceptRequest, ContractGrpc, ContractListResponse, ContractProposeRequest,
    ContractRedeemRequest, ContractRejectRequest, ContractRequest, ContractResponse,
    ContractRpc, ContractTerminateRequest, EventMeta as ContractEventMeta, ListRequest
        as ContractListRequest, PlayerIdentifier as ContractPlayerIdentifier,
};
pub use creature_service::{
    CreatureConfigProto, CreatureListResponse, CreatureRequest, CreatureRpc,
    CreatureServiceGrpc, Empty, ListRequest, ReloadResponse,
};
pub use core_pod_service::{
    CorePodGrpc, CorePodRpc, EventMeta as CorePodEventMeta, PodEnterRequest, PodEnterResponse,
    PodExitResponse, PodState, PodTickResult,
};
pub use dglab_service::{
    DglabGrpc, DglabRpc, EventMeta as DglabEventMeta, ListRequest as DglabListRequest,
    SetStrengthRequest, SetStrengthResponse, StrengthResponse, TokenRequest, TokenResponse,
    TokenListResponse,
};
pub use environment_service::{
    BlockPos, EnvironmentEffectRequest, EnvironmentEffectResponse, EnvironmentModifierEntry,
    EnvironmentModifiers, EnvironmentRpc, EnvironmentServiceGrpc, PlayerIdentifier
        as EnvironmentPlayerIdentifier,
};
pub use hostile_mob_service::{
    CreatureIdRequest, DropChanceResponse, EventMeta as HostileMobEventMeta, HostileMobGrpc,
    HostileMobRpc,
};
pub use player_state_service::{
    DamageRequest, DamageResponse, HungerRequest, PlayerIdentifier, PlayerState,
    PlayerStateGrpc, PlayerStateRpc, PlayerStateUpdate, PleasureRequest,
};

/// Placeholder kept for callers that may still depend on a no-op
/// symbol. New code should use the typed re-exports above.
pub fn placeholder() {}