use crate::model::DecodeError;

/// A configuration, protocol, or transport failure. Diagnostics omit remote bodies and credentials.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct Error(#[from] pub(crate) Failure);

#[derive(Debug, thiserror::Error)]
pub(crate) enum Failure {
    #[error("invalid configuration: {0}")]
    Config(&'static str),
    #[error("{operation}: HTTP {status}")]
    Http {
        operation: &'static str,
        status: u16,
    },
    #[error("{operation}: transport failed")]
    Transport {
        operation: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("{0}")]
    Protocol(&'static str),
    #[error("job declaration: {0}")]
    Decode(#[from] DecodeError),
    #[error("RSA identity generation failed")]
    Rsa(#[from] rsa::errors::Error),
    #[error("RSA identity encoding failed")]
    Key(#[from] rsa::pkcs1::Error),
    #[error("JWT assertion failed")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("background task failed")]
    Task(#[from] tokio::task::JoinError),
    #[error("shutdown signal failed")]
    Signal(#[from] std::io::Error),
    #[error("job was cancelled")]
    Cancelled,
}

impl Failure {
    pub fn is_status(&self, expected: u16) -> bool {
        matches!(self, Self::Http { status, .. } if *status == expected)
    }

    pub fn transient(&self) -> bool {
        matches!(
            self,
            Self::Transport { .. }
                | Self::Http {
                    status: 429 | 500..=599,
                    ..
                }
        )
    }
}

pub(crate) type Result<T> = std::result::Result<T, Failure>;
