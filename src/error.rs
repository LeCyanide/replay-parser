#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o: {0}")]
    Io(#[source] std::io::Error),
    #[error("file ends inside the {what}: needs {needed} bytes, {available} left")]
    Truncated { what: &'static str, needed: usize, available: usize },
    #[error("replay declares no blocks, so it has no arena info")]
    NoMetaBlock,
    #[error("arena info block is not UTF-8")]
    MetaNotUtf8,
    #[error("arena info block is not the expected JSON: {0}")]
    MetaJson(#[source] serde_json::Error),
    #[error("the packet stream inflated to nothing")]
    EmptyStream,
    #[error("unparseable client version {0:?}")]
    BadVersion(String),
}
