use serde_json::json;
use thiserror::Error;

pub const EMPTY_RESULT_EXIT_CODE: i32 = 3;
pub type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("{message}")]
    Usage { message: String, json: bool },
    #[error("{message}")]
    Validation { message: String, json: bool },
    #[error("{message}")]
    EmptyResult { message: String, json: bool },
    #[error(transparent)]
    Operational(#[from] anyhow::Error),
}

impl CliError {
    pub fn json_mode(&self) -> bool {
        match self {
            Self::Usage { json, .. }
            | Self::Validation { json, .. }
            | Self::EmptyResult { json, .. } => *json,
            Self::Operational(_) => false,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Usage { .. } => "usage_error",
            Self::Validation { .. } => "validation_error",
            Self::EmptyResult { .. } => "empty_result",
            Self::Operational(_) => "operational_error",
        }
    }
}

impl From<rusqlite::Error> for CliError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Operational(value.into())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(value: serde_json::Error) -> Self {
        Self::Operational(value.into())
    }
}

pub fn exit_code(err: &CliError) -> i32 {
    match err {
        CliError::Usage { .. } => 2,
        CliError::EmptyResult { .. } => EMPTY_RESULT_EXIT_CODE,
        _ => 1,
    }
}

pub fn emit_error(json_output: bool, err: &CliError, exit_code: i32) {
    if json_output {
        let payload = json!({
            "error": {
                "code": err.code(),
                "message": err.to_string(),
                "exit_code": exit_code,
            }
        });
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| {
                "{\"error\":{\"code\":\"internal\",\"message\":\"failed to render error\"}}"
                    .to_string()
            })
        );
    } else {
        eprintln!("Error: {err}");
    }
}

pub fn validation<T>(message: &str) -> CliResult<T> {
    Err(CliError::Validation {
        message: message.to_string(),
        json: false,
    })
}
