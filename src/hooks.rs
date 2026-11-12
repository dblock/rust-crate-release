use crate::Result;
use crate::execution::ExecutionContext;
use crate::workflow::Step;
use std::sync::Arc;

pub trait LifecycleHook<Input, Output>: Send + Sync {
    fn id(&self) -> &str;
    fn before(&self, _input: &Input, _execution: &ExecutionContext) -> Result {
        Ok(())
    }
    fn after(&self, _output: &Output, _execution: &ExecutionContext) -> Result {
        Ok(())
    }
}

pub struct LifecycleHooks<I, O> {
    hooks: Vec<Arc<dyn LifecycleHook<I, O>>>,
}

impl<I, O> Default for LifecycleHooks<I, O> {
    fn default() -> Self {
        Self { hooks: Vec::new() }
    }
}

impl<I, O> Clone for LifecycleHooks<I, O> {
    fn clone(&self) -> Self {
        Self {
            hooks: self.hooks.clone(),
        }
    }
}

impl<I, O> LifecycleHooks<I, O> {
    pub fn with<H: LifecycleHook<I, O> + 'static>(&self, hook: H) -> Self {
        let mut hooks = self.hooks.clone();
        hooks.push(Arc::new(hook));
        Self { hooks }
    }

    pub fn wrap<S: Step<I, Output = O>>(self, step: S) -> HookedStep<S, I, O> {
        HookedStep { step, hooks: self }
    }
}

pub struct HookedStep<S, I, O> {
    step: S,
    hooks: LifecycleHooks<I, O>,
}

impl<S, I, O> Step<I> for HookedStep<S, I, O>
where
    S: Step<I, Output = O>,
{
    type Output = O;
    fn id(&self) -> &str {
        self.step.id()
    }
    fn run(&self, input: I, execution: &ExecutionContext) -> Result<O> {
        for (index, hook) in self.hooks.hooks.iter().enumerate() {
            hook.before(&input, execution).map_err(|error| {
                error.context(format!(
                    "{} before hook {} at index {index}",
                    self.id(),
                    hook.id()
                ))
            })?;
        }
        let output = self.step.run(input, execution)?;
        for (index, hook) in self.hooks.hooks.iter().enumerate() {
            hook.after(&output, execution).map_err(|error| {
                error.context(format!(
                    "{} after hook {} at index {index}",
                    self.id(),
                    hook.id()
                ))
            })?;
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, Workflow};
    use std::sync::Mutex;

    struct Add(Arc<Mutex<Vec<String>>>);
    impl Step<usize> for Add {
        type Output = usize;
        fn id(&self) -> &str {
            "add"
        }
        fn run(&self, input: usize, _: &ExecutionContext) -> Result<usize> {
            self.0.lock().unwrap().push("step".into());
            Ok(input + 1)
        }
    }
    struct Hook {
        id: &'static str,
        events: Arc<Mutex<Vec<String>>>,
        fail: bool,
        fail_after: bool,
    }
    impl LifecycleHook<usize, usize> for Hook {
        fn id(&self) -> &str {
            self.id
        }
        fn before(&self, input: &usize, _: &ExecutionContext) -> Result {
            self.events
                .lock()
                .unwrap()
                .push(format!("{} before {input}", self.id));
            if self.fail {
                Err(Error::invalid("hook original"))
            } else {
                Ok(())
            }
        }
        fn after(&self, output: &usize, _: &ExecutionContext) -> Result {
            self.events
                .lock()
                .unwrap()
                .push(format!("{} after {output}", self.id));
            if self.fail_after {
                Err(Error::invalid("after hook original"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn ordered_hooks_wrap_typed_inputs_and_outputs_without_mutating_registry() -> Result {
        let events = Arc::new(Mutex::new(Vec::new()));
        let original = LifecycleHooks::default().with(Hook {
            id: "one",
            events: events.clone(),
            fail: false,
            fail_after: false,
        });
        let extended = original.with(Hook {
            id: "two",
            events: events.clone(),
            fail: false,
            fail_after: false,
        });
        assert_eq!(original.hooks.len(), 1);
        let workflow = Workflow::start().then(extended.wrap(Add(events.clone())))?;
        assert_eq!(workflow.run(3)?, 4);
        assert_eq!(
            *events.lock().unwrap(),
            [
                "one before 3",
                "two before 3",
                "step",
                "one after 4",
                "two after 4"
            ]
        );
        Ok(())
    }

    #[test]
    fn hook_failure_stops_operation_and_retains_source_with_stage_timing_id_and_index() -> Result {
        use std::error::Error as _;
        let events = Arc::new(Mutex::new(Vec::new()));
        let step = LifecycleHooks::default()
            .with(Hook {
                id: "broken",
                events: events.clone(),
                fail: true,
                fail_after: false,
            })
            .wrap(Add(events.clone()));
        let error = Workflow::start().then(step)?.run(0).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("add before hook broken at index 0")
        );
        assert_eq!(error.source().unwrap().to_string(), "hook original");
        assert_eq!(events.lock().unwrap().len(), 1);
        Ok(())
    }

    #[test]
    fn after_hook_failure_reports_completed_operation_without_running_later_hooks() -> Result {
        use std::error::Error as _;
        let events = Arc::new(Mutex::new(Vec::new()));
        let step = LifecycleHooks::default()
            .with(Hook {
                id: "broken",
                events: events.clone(),
                fail: false,
                fail_after: true,
            })
            .with(Hook {
                id: "later",
                events: events.clone(),
                fail: false,
                fail_after: false,
            })
            .wrap(Add(events.clone()));
        let error = Workflow::start().then(step)?.run(0).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("add after hook broken at index 0")
        );
        assert_eq!(error.source().unwrap().to_string(), "after hook original");
        assert_eq!(
            *events.lock().unwrap(),
            [
                "broken before 0",
                "later before 0",
                "step",
                "broken after 1"
            ]
        );
        Ok(())
    }
}
