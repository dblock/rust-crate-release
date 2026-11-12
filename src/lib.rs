#![doc = include_str!("../README.md")]

pub mod changelog;
mod error;
pub mod execution;
pub mod hooks;
pub mod markdown;
mod release;
pub mod semantic;
pub mod workflow;
pub use error::{Error, Result};
pub use release::Release;
pub use workflow::{RunOptions, Step, Workflow, WorkflowObserver};
