# Typed Workflows

## Compose Steps

Implement `Step<Input>` with an associated output type, then compose it with `Workflow::start().then(...)`. Construction rejects empty or duplicate identifiers; Rust rejects incompatible adjacent types. Workflows are immutable and can be reused; every run gets fresh execution history.

```rust
use rust_crate_release::{execution::ExecutionContext, Result, Step, Workflow};

struct Length;

impl Step<String> for Length {
    type Output = usize;

    fn id(&self) -> &str {
        "length"
    }

    fn run(&self, input: String, _: &ExecutionContext) -> Result<usize> {
        Ok(input.len())
    }
}

fn main() -> Result {
    let workflow = Workflow::start().then(Length)?;
    assert_eq!(workflow.run("crate".to_owned())?, 5);
    Ok(())
}
```

The runner is sequential and does not retry steps. Type compatibility does not prove that custom code preserves release safety; replacing providers or omitting guards transfers responsibility to the caller.

## Observe Execution

Pass `RunOptions` with `Arc<dyn WorkflowObserver>` values to `run_with`. Before/after observers receive immutable snapshots with the current step, prior observed history, workflow identifiers, and explicit run metadata. A missing step is unknown, not automatically skipped. `StepRecord` separates operation outcome from conclusion: an after-observer failure can fail a conclusion after a successful operation.

Failure observers are all attempted, even if one fails. Their diagnostic failures are written to stderr without replacing the original operation error. An observer failure stops downstream steps; it never retries a side effect. Snapshots contain no environment dump, credentials, callbacks, or live platform clients.

## Register Typed Hooks

`LifecycleHook<Input, Output>` receives a borrowed input before the operation and a borrowed output afterward, plus execution metadata. Add hook instances with `LifecycleHooks::default().with(hook)`, then `wrap(step)` before composing. Registries are immutable; extending one does not change an earlier registry. Before and after hooks run in registration order, not reverse stack order.

Hook errors stop execution and retain the original error as their source, adding the stage identifier, before/after timing, hook identifier, and registration index. Hooks are trusted Rust code, not a sandbox: borrowed inputs prevent ordinary field mutation but cannot prevent arbitrary filesystem access. Built-in release steps enforce file/index edit boundaries independently of these generic adapters.

## Select an Execution Provider

`LocalExecutionProvider` reports local metadata and summaries to stderr. `GitHubExecutionProvider` captures an explicit allowlist of Actions fields and appends summaries to the configured step-summary file. It requires `workflow_dispatch` on the configured default branch when validated. `execution_provider(default_branch)` detects GitHub only when `GITHUB_ACTIONS=true`; callers can instead supply a provider explicitly.

Execution providers are independent of registry and hosted-release providers. A GitHub execution snapshot does not select or authenticate a registry, and constructing an execution provider does not publish anything.
