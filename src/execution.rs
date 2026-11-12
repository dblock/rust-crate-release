use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    Success,
    Failure,
    Skipped,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub id: String,
    pub outcome: StepOutcome,
    pub conclusion: StepOutcome,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunMetadata {
    pub platform: String,
    pub run_id: Option<String>,
    pub run_url: Option<String>,
    pub repository: Option<String>,
    pub attempt: Option<String>,
    pub reference: Option<String>,
    pub sha: Option<String>,
    pub job: Option<String>,
    pub workflow_name: Option<String>,
    pub event: Option<String>,
    #[serde(default)]
    pub external_steps: Vec<StepRecord>,
}

/// A point-in-time value, not a live platform client or an environment dump.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub current_step: Option<String>,
    pub history: Vec<StepRecord>,
    pub workflow: Vec<String>,
    pub run: RunMetadata,
}

pub trait ExecutionProvider: Send + Sync {
    fn metadata(&self) -> RunMetadata;
    fn validate(&self) -> Result {
        Ok(())
    }
    fn summary(&self, markdown: &str) -> Result;
}

#[derive(Clone, Debug, Default)]
pub struct LocalExecutionProvider;

impl ExecutionProvider for LocalExecutionProvider {
    fn metadata(&self) -> RunMetadata {
        RunMetadata {
            platform: "local".into(),
            ..RunMetadata::default()
        }
    }

    fn summary(&self, markdown: &str) -> Result {
        eprintln!("{markdown}");
        Ok(())
    }
}

/// Captures only explicitly supported, non-secret Actions fields.
#[derive(Clone, Debug)]
pub struct GitHubExecutionProvider {
    metadata: RunMetadata,
    event: Option<String>,
    reference: Option<String>,
    default_branch: String,
    summary: Option<PathBuf>,
}

impl GitHubExecutionProvider {
    pub fn from_environment(default_branch: impl Into<String>) -> Self {
        Self::from_values(std::env::vars().collect(), default_branch.into())
    }

    pub fn from_values(values: BTreeMap<String, String>, default_branch: String) -> Self {
        let value = |key: &str| values.get(key).filter(|v| !v.is_empty()).cloned();
        let server = value("GITHUB_SERVER_URL").unwrap_or_else(|| "https://github.com".into());
        let repository = value("GITHUB_REPOSITORY");
        let run_id = value("GITHUB_RUN_ID");
        let run_url = repository
            .as_ref()
            .zip(run_id.as_ref())
            .map(|(repo, id)| format!("{server}/{repo}/actions/runs/{id}"));
        Self {
            metadata: RunMetadata {
                platform: "github".into(),
                repository,
                run_id,
                run_url,
                attempt: value("GITHUB_RUN_ATTEMPT"),
                reference: value("GITHUB_REF"),
                sha: value("GITHUB_SHA"),
                job: value("GITHUB_JOB"),
                workflow_name: value("GITHUB_WORKFLOW"),
                event: value("GITHUB_EVENT_NAME"),
                external_steps: external_steps(value("RCR_STEPS").as_deref()).unwrap_or_default(),
            },
            event: value("GITHUB_EVENT_NAME"),
            reference: value("GITHUB_REF"),
            default_branch,
            summary: value("GITHUB_STEP_SUMMARY").map(PathBuf::from),
        }
    }
}

/// Projects the actual Actions snapshot onto outcomes only; outputs are discarded.
pub fn external_steps(source: Option<&str>) -> Result<Vec<StepRecord>> {
    #[derive(Deserialize)]
    struct Observed {
        outcome: StepOutcome,
        conclusion: StepOutcome,
    }
    let Some(source) = source.filter(|source| !source.is_empty()) else {
        return Ok(Vec::new());
    };
    if source.len() > 1024 * 1024 {
        return Err(Error::invalid("Actions step snapshot is too large"));
    }
    let steps: BTreeMap<String, Observed> = serde_json::from_str(source)
        .map_err(|_| Error::invalid("invalid Actions step outcomes"))?;
    Ok(steps
        .into_iter()
        .map(|(id, step)| StepRecord {
            id,
            outcome: step.outcome,
            conclusion: step.conclusion,
        })
        .collect())
}

/// Trusted direct-argv callbacks receive only the credential-free execution snapshot.
pub struct CommandCallbacks {
    pub root: PathBuf,
    pub success: Option<Vec<String>>,
    pub failure: Option<Vec<String>>,
}

impl CommandCallbacks {
    pub fn notify(&self, success: bool, execution: &ExecutionContext) -> Result {
        let command = if success {
            &self.success
        } else {
            &self.failure
        };
        let Some(command) = command else {
            return Ok(());
        };
        let (program, args) = command
            .split_first()
            .filter(|(program, _)| !program.is_empty())
            .ok_or_else(|| Error::invalid("callback requires nonempty program argv"))?;
        let context = serde_json::to_string(execution)?;
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        let output = crate::command::run_with_env(
            &self.root,
            program,
            &args,
            &[
                ("RCR_STATUS", if success { "success" } else { "failure" }),
                ("RCR_EXECUTION_CONTEXT", &context),
            ],
        )?;
        std::io::stderr().write_all(&output.stdout)?;
        std::io::stderr().write_all(&output.stderr)?;
        Ok(())
    }
}

impl crate::WorkflowObserver for CommandCallbacks {
    fn success(&self, execution: &ExecutionContext) -> Result {
        self.notify(true, execution)
    }
    fn failure(&self, execution: &ExecutionContext, _: &Error) -> Result {
        self.notify(false, execution)
    }
}

impl ExecutionProvider for GitHubExecutionProvider {
    fn metadata(&self) -> RunMetadata {
        self.metadata.clone()
    }

    fn validate(&self) -> Result {
        if self.event.as_deref() != Some("workflow_dispatch")
            || self.reference.as_deref()
                != Some(format!("refs/heads/{}", self.default_branch).as_str())
        {
            return Err(Error::invalid(
                "GitHub releases require manual dispatch on the configured default branch",
            ));
        }
        Ok(())
    }

    fn summary(&self, markdown: &str) -> Result {
        if let Some(path) = &self.summary {
            writeln!(
                OpenOptions::new().create(true).append(true).open(path)?,
                "{markdown}"
            )?;
        }
        Ok(())
    }
}

pub fn execution_provider(default_branch: &str) -> Box<dyn ExecutionProvider> {
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
        Box::new(GitHubExecutionProvider::from_environment(default_branch))
    } else {
        Box::new(LocalExecutionProvider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> BTreeMap<String, String> {
        [
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REF", "refs/heads/master"),
            ("GITHUB_REPOSITORY", "owner/project"),
            ("GITHUB_RUN_ID", "42"),
            ("GITHUB_RUN_ATTEMPT", "2"),
            ("SECRET", "must-not-appear"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }

    #[test]
    fn external_outcomes_discard_outputs_and_preserve_continued_failures_and_cancellation() -> Result
    {
        let steps = external_steps(Some(
            r#"{
            "test":{"outcome":"failure","conclusion":"success","outputs":{"token":"must-not-appear"}},
            "cancelled":{"outcome":"cancelled","conclusion":"cancelled"},
            "skipped":{"outcome":"skipped","conclusion":"skipped"}
        }"#,
        ))?;
        assert_eq!(steps[2].outcome, StepOutcome::Failure);
        assert_eq!(steps[2].conclusion, StepOutcome::Success);
        assert_eq!(steps[0].outcome, StepOutcome::Cancelled);
        assert!(!serde_json::to_string(&steps)?.contains("must-not-appear"));
        assert!(
            external_steps(Some(
                r#"{"test":{"outcome":"unknown","conclusion":"success"}}"#
            ))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn github_metadata_is_an_explicit_snapshot_without_environment_secrets() -> Result {
        let provider = GitHubExecutionProvider::from_values(values(), "master".into());
        provider.validate()?;
        let snapshot = serde_json::to_string(&provider.metadata())?;
        assert!(snapshot.contains("https://github.com/owner/project/actions/runs/42"));
        assert!(!snapshot.contains("must-not-appear"));
        let mut wrong = values();
        wrong.insert("GITHUB_EVENT_NAME".into(), "pull_request".into());
        assert!(
            GitHubExecutionProvider::from_values(wrong, "master".into())
                .validate()
                .is_err()
        );
        assert!(
            GitHubExecutionProvider::from_values(values(), "other".into())
                .validate()
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn summaries_append_to_only_the_explicit_output() -> Result {
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join("summary");
        let mut values = values();
        values.insert("GITHUB_STEP_SUMMARY".into(), path.to_str().unwrap().into());
        let provider = GitHubExecutionProvider::from_values(values, "master".into());
        provider.summary("One")?;
        provider.summary("Two")?;
        assert_eq!(std::fs::read_to_string(path)?, "One\nTwo\n");
        GitHubExecutionProvider::from_values(BTreeMap::new(), "master".into())
            .summary("Ignored")?;
        Ok(())
    }
}
