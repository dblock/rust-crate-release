#![doc = include_str!("../docs/workflows.md")]

use crate::execution::{ExecutionContext, RunMetadata, StepOutcome, StepRecord};
use crate::{Error, Result};
use std::sync::Arc;

/// Adjacent steps compose only when their input/output types agree.
pub trait Step<Input>: Send + Sync {
    type Output;
    fn id(&self) -> &str;
    fn run(&self, input: Input, execution: &ExecutionContext) -> Result<Self::Output>;
}

pub trait WorkflowObserver: Send + Sync {
    fn before(&self, _execution: &ExecutionContext) -> Result {
        Ok(())
    }
    fn after(&self, _execution: &ExecutionContext) -> Result {
        Ok(())
    }
    fn failure(&self, _execution: &ExecutionContext, _error: &Error) -> Result {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct RunOptions {
    pub metadata: RunMetadata,
    pub observers: Vec<Arc<dyn WorkflowObserver>>,
}

struct Invocation {
    execution: ExecutionContext,
    observers: Vec<Arc<dyn WorkflowObserver>>,
}

type Runner<I, O> = dyn Fn(I, &mut Invocation) -> Result<O> + Send + Sync;

/// An immutable sequential composition, reusable across independent invocations.
///
/// ```compile_fail
/// use rust_crate_release::{execution::ExecutionContext, Result, Step, Workflow};
/// struct Length;
/// impl Step<String> for Length {
///     type Output = usize;
///     fn id(&self) -> &str { "length" }
///     fn run(&self, value: String, _: &ExecutionContext) -> Result<usize> {
///         Ok(value.len())
///     }
/// }
/// // The previous output is usize, but Length requires String.
/// let _ = Workflow::<usize, usize>::start().then(Length);
/// ```
pub struct Workflow<Input, Output> {
    runner: Arc<Runner<Input, Output>>,
    steps: Vec<String>,
}

impl<I, O> Clone for Workflow<I, O> {
    fn clone(&self) -> Self {
        Self {
            runner: self.runner.clone(),
            steps: self.steps.clone(),
        }
    }
}

impl<I: 'static> Workflow<I, I> {
    pub fn start() -> Self {
        Self {
            runner: Arc::new(|input, _| Ok(input)),
            steps: Vec::new(),
        }
    }
}

impl<I: 'static, O: 'static> Workflow<I, O> {
    pub fn then<S>(self, step: S) -> Result<Workflow<I, S::Output>>
    where
        S: Step<O> + 'static,
        S::Output: 'static,
    {
        let id = step.id().to_owned();
        if id.trim().is_empty() || self.steps.contains(&id) {
            return Err(Error::invalid(
                "step identifiers must be nonempty and unique",
            ));
        }
        let mut steps = self.steps;
        steps.push(id.clone());
        let previous = self.runner;
        Ok(Workflow {
            steps,
            runner: Arc::new(move |input, invocation| {
                let input = previous(input, invocation)?;
                invocation.execution.current_step = Some(id.clone());
                for observer in &invocation.observers {
                    if let Err(error) = observer.before(&invocation.execution) {
                        invocation.execution.history.push(StepRecord {
                            id: id.clone(),
                            outcome: StepOutcome::Skipped,
                            conclusion: StepOutcome::Failure,
                        });
                        return Err(error);
                    }
                }
                let result = step.run(input, &invocation.execution);
                let outcome = if result.is_ok() {
                    StepOutcome::Success
                } else {
                    StepOutcome::Failure
                };
                invocation.execution.history.push(StepRecord {
                    id: id.clone(),
                    outcome: outcome.clone(),
                    conclusion: outcome,
                });
                let output = result?;
                for observer in &invocation.observers {
                    if let Err(error) = observer.after(&invocation.execution) {
                        invocation.execution.history.last_mut().unwrap().conclusion =
                            StepOutcome::Failure;
                        return Err(error);
                    }
                }
                Ok(output)
            }),
        })
    }

    pub fn steps(&self) -> &[String] {
        &self.steps
    }

    pub fn run(&self, input: I) -> Result<O> {
        self.run_with(input, RunOptions::default())
    }

    pub fn run_with(&self, input: I, options: RunOptions) -> Result<O> {
        let mut invocation = Invocation {
            execution: ExecutionContext {
                workflow: self.steps.clone(),
                run: options.metadata,
                ..ExecutionContext::default()
            },
            observers: options.observers,
        };
        let result = (self.runner)(input, &mut invocation);
        if let Err(error) = &result {
            for observer in &invocation.observers {
                if let Err(report_error) = observer.failure(&invocation.execution, error) {
                    eprintln!("failure observer failed: {report_error}");
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

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
    struct Double;
    impl Step<usize> for Double {
        type Output = usize;
        fn id(&self) -> &str {
            "double"
        }
        fn run(&self, input: usize, _: &ExecutionContext) -> Result<usize> {
            Ok(input * 2)
        }
    }
    struct Fail;
    impl Step<usize> for Fail {
        type Output = usize;
        fn id(&self) -> &str {
            "fail"
        }
        fn run(&self, _: usize, _: &ExecutionContext) -> Result<usize> {
            Err(Error::invalid("original"))
        }
    }
    #[derive(Default)]
    struct Observer {
        events: Mutex<Vec<ExecutionContext>>,
        fail_report: bool,
        fail_before: bool,
        fail_after: bool,
    }
    impl WorkflowObserver for Observer {
        fn before(&self, execution: &ExecutionContext) -> Result {
            self.events.lock().unwrap().push(execution.clone());
            if self.fail_before {
                Err(Error::invalid("before observer"))
            } else {
                Ok(())
            }
        }
        fn after(&self, execution: &ExecutionContext) -> Result {
            self.events.lock().unwrap().push(execution.clone());
            if self.fail_after {
                Err(Error::invalid("after observer"))
            } else {
                Ok(())
            }
        }
        fn failure(&self, execution: &ExecutionContext, _: &Error) -> Result {
            self.events.lock().unwrap().push(execution.clone());
            if self.fail_report {
                Err(Error::invalid("reporter"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn typed_steps_compose_and_each_invocation_has_independent_snapshots() -> Result {
        let workflow = Workflow::start().then(Length)?.then(Double)?;
        let observer = Arc::new(Observer::default());
        let options = RunOptions {
            observers: vec![observer.clone()],
            ..RunOptions::default()
        };
        assert_eq!(workflow.run_with("abc".into(), options.clone())?, 6);
        assert_eq!(workflow.run_with("a".into(), options)?, 2);
        let events = observer.events.lock().unwrap();
        assert!(events[0].history.is_empty());
        assert!(events[4].history.is_empty());
        assert_eq!(events[3].history.len(), 2);
        assert_eq!(events[0].current_step.as_deref(), Some("length"));
        Ok(())
    }

    #[test]
    fn errors_stop_downstream_and_all_failure_reporters_preserve_original_error() -> Result {
        let first = Arc::new(Observer {
            fail_report: true,
            ..Observer::default()
        });
        let second = Arc::new(Observer::default());
        let workflow = Workflow::start().then(Fail)?.then(Double)?;
        let error = workflow
            .run_with(
                1,
                RunOptions {
                    observers: vec![first, second.clone()],
                    ..RunOptions::default()
                },
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "original");
        let events = second.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].history[0].outcome, StepOutcome::Failure);
        assert_eq!(events[1].current_step.as_deref(), Some("fail"));
        Ok(())
    }

    #[test]
    fn duplicate_step_identifiers_fail_before_execution() -> Result {
        assert!(
            Workflow::<usize, usize>::start()
                .then(Double)?
                .then(Double)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn observer_failures_distinguish_skipped_and_successful_operation_outcomes() -> Result {
        for before in [true, false] {
            let failing = Arc::new(Observer {
                fail_before: before,
                fail_after: !before,
                ..Observer::default()
            });
            let reporter = Arc::new(Observer::default());
            let workflow = Workflow::start().then(Double)?.then(Fail)?;
            assert!(
                workflow
                    .run_with(
                        1,
                        RunOptions {
                            observers: vec![failing, reporter.clone()],
                            ..RunOptions::default()
                        }
                    )
                    .is_err()
            );
            let snapshots = reporter.events.lock().unwrap();
            let failure = snapshots.last().unwrap();
            assert_eq!(failure.history.len(), 1);
            assert_eq!(failure.history[0].conclusion, StepOutcome::Failure);
            assert_eq!(
                failure.history[0].outcome,
                if before {
                    StepOutcome::Skipped
                } else {
                    StepOutcome::Success
                }
            );
        }
        Ok(())
    }
}
