use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config parse error: {0}")]
    Config(#[from] toml::de::Error),

    #[error("config serialize error: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    #[error("dbus error: {0}")]
    Zbus(#[from] zbus::Error),

    #[error("no supported backend detected for {0}")]
    NoBackend(&'static str),

    #[error("sysfs write to {path} failed: {source}")]
    SysfsWrite {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("conflicting service active: {0} - stop it first or disable this check in config")]
    ConflictingService(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for zbus::fdo::Error {
    fn from(e: Error) -> Self {
        zbus::fdo::Error::Failed(e.to_string())
    }
}
