use std::io;

/// A failed validation or operation. Errors never authorize retrying publication.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Version(#[from] semver::Error),
    #[error("{context}: {source}")]
    Operation {
        context: String,
        #[source]
        source: Box<Error>,
    },
}

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn context(self, context: impl Into<String>) -> Self {
        Self::Operation {
            context: context.into(),
            source: Box::new(self),
        }
    }
}

pub type Result<T = ()> = std::result::Result<T, Error>;
