use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct ContextIndexStats {
    pub files_indexed: u64,
    pub bytes_hashed: u64,
    pub last_scan_at: Option<Instant>,
    pub last_change_at: Option<Instant>,
    pub currently_scanning: bool,
    pub scan_progress_done: u64,
    pub scan_progress_total: u64,
    pub enabled: bool,
    pub chunks_indexed: u64,
    pub files_chunked: u64,
}

impl ContextIndexStats {
    pub fn to_proto(&self) -> proto::ContextIndexStats {
        proto::ContextIndexStats {
            files_indexed: self.files_indexed,
            bytes_hashed: self.bytes_hashed,
            last_scan_at_ms: self
                .last_scan_at
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0),
            last_change_at_ms: self
                .last_change_at
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0),
            currently_scanning: self.currently_scanning,
            scan_progress_done: self.scan_progress_done,
            scan_progress_total: self.scan_progress_total,
            enabled: self.enabled,
            chunks_indexed: self.chunks_indexed,
            files_chunked: self.files_chunked,
        }
    }

    pub fn from_proto(msg: &proto::ContextIndexStats) -> Self {
        Self {
            files_indexed: msg.files_indexed,
            bytes_hashed: msg.bytes_hashed,
            last_scan_at: if msg.last_scan_at_ms > 0 {
                Some(Instant::now() - std::time::Duration::from_millis(msg.last_scan_at_ms))
            } else {
                None
            },
            last_change_at: if msg.last_change_at_ms > 0 {
                Some(Instant::now() - std::time::Duration::from_millis(msg.last_change_at_ms))
            } else {
                None
            },
            currently_scanning: msg.currently_scanning,
            scan_progress_done: msg.scan_progress_done,
            scan_progress_total: msg.scan_progress_total,
            enabled: msg.enabled,
            chunks_indexed: msg.chunks_indexed,
            files_chunked: msg.files_chunked,
        }
    }
}
