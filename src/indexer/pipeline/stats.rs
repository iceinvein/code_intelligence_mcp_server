#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct IndexRunStats {
    pub files_scanned: usize,
    pub files_indexed: usize,
    pub symbols_indexed: usize,
    pub files_skipped: usize,
    pub files_unchanged: usize,
    pub files_deleted: usize,
}
