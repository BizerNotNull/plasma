use std::fmt;

/// UI, argument and API failures. Control errors are shown in the window;
/// process-level diagnostics go to stderr from the binary, never from libraries.
#[derive(Debug)]
pub enum Error {
    Synth(plasma_api::Error),
    Message(&'static str),
    Owned(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Synth(error) => write!(f, "{error}"),
            Self::Message(message) => f.write_str(message),
            Self::Owned(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Synth(error) => Some(error),
            _ => None,
        }
    }
}

impl From<plasma_api::Error> for Error {
    fn from(error: plasma_api::Error) -> Self {
        Self::Synth(error)
    }
}

impl From<&'static str> for Error {
    fn from(message: &'static str) -> Self {
        Self::Message(message)
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Owned(message)
    }
}

pub fn log_error(error: impl fmt::Display) {
    eprintln!("plasma-ui: {error}");
}
