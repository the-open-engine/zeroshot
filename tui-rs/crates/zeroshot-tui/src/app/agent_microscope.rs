use crate::protocol::ClusterLogLine;
use crate::ui::shared::TimeIndexedBuffer;

pub const MAX_LOG_LINES: usize = 1000;

#[derive(Debug, Clone)]
pub struct State {
    pub logs_time: TimeIndexedBuffer<ClusterLogLine>,
    pub log_drop_seq: u64,
    pub log_subscription: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            logs_time: TimeIndexedBuffer::new(MAX_LOG_LINES),
            log_drop_seq: 0,
            log_subscription: None,
            role: None,
            status: None,
        }
    }
}

impl State {
    pub fn push_log_lines(&mut self, mut lines: Vec<ClusterLogLine>, dropped_count: Option<i64>) {
        let mut to_push = Vec::new();
        if let Some(count) = dropped_count {
            if count > 0 {
                let line = ClusterLogLine {
                    id: format!("dropped-{}", self.log_drop_seq),
                    timestamp: lines.first().map(|line| line.timestamp).unwrap_or(0),
                    text: format!("[dropped {} log lines]", count),
                    agent: None,
                    role: None,
                    sender: None,
                };
                self.log_drop_seq = self.log_drop_seq.saturating_add(1);
                to_push.push(line);
            }
        }

        to_push.append(&mut lines);
        self.logs_time.push_many(to_push);
    }
}
