use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error at {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("invalid JSON in {path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
    #[error("{0} is not a .wall library (no wall.json)")]
    NotALibrary(PathBuf),
    #[error("{path} uses format version {found}; this build reads up to {supported}")]
    TooNew { path: PathBuf, found: u32, supported: u32 },
    #[error("invalid content hash: {0}")]
    BadHash(String),
    #[error("invalid file extension: {0}")]
    BadExtension(String),
    #[error("{0} already exists")]
    AlreadyExists(PathBuf),
    #[error("{0} is not in the trash")]
    NotInTrash(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) trait IoContext<T> {
    fn at(self, path: &std::path::Path) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: &std::path::Path) -> Result<T> {
        self.map_err(|source| Error::Io { path: path.to_path_buf(), source })
    }
}
