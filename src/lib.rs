#![doc = include_str!("../README.md")]

pub mod actions;
pub mod changelog;
pub mod cli;
pub mod command;
pub mod engine;
mod error;
pub mod execution;
pub mod git;
pub mod hooks;
pub mod hosting;
pub mod markdown;
pub mod package;
pub mod registry;
mod release;
pub mod semantic;
pub mod store;
pub mod testing;
pub mod workflow;
pub use error::{Error, Result};
pub use release::Release;
pub use workflow::{RunOptions, Step, Workflow, WorkflowObserver};
