//! Opt-in acceptance lanes that talk to the REAL world (the public npm
//! registry, real DSH servers). They are deliberately outside `release_e2e`:
//! that module is the deterministic CI gate, and a lane that needs the network
//! must never be swept into it. Each lane documents its own opt-in switch.

pub(crate) mod real_dsh;
pub(crate) mod real_runtime;
