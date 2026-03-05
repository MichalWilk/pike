use thiserror::Error;

#[derive(Error, Debug)]
pub enum PikeError {
    #[error("command `{cmd}` failed (exit {exit_code}): {stderr}")]
    CommandFailed {
        cmd: String,
        exit_code: i32,
        stderr: String,
    },

    #[error("failed to parse {source_name} output: {detail}")]
    Parse { source_name: String, detail: String },

    #[error("'{name}' not found in {source_name}")]
    NotFound { name: String, source_name: String },

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid input: {0}")]
    Validation(String),

    #[error("{0}")]
    Other(String),
}
