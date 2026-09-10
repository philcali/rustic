//! Plan step (pure) + Apply step (agent) for the CLI.
//!
//! The concrete plan-building lives in the shared `pandemic_common::apply`
//! module (so `pandemic-rest` drives the identical plan without drifting); the
//! privileged apply sequence now lives in the agent (`pandemic-agent::apply`),
//! reached through `ApplyInfection`/`ApplyDeployment`. This module keeps a
//! single import site for the CLI's infection/deployment commands.

pub use pandemic_common::apply::{build_plan, build_plan_from_spec, parse_set_args};
pub use pandemic_protocol::{ApplyDeploymentInfection, Plan};
