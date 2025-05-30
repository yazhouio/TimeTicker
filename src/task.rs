use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
pub enum TaskType {
    Duration(Duration),    // 时间段类型
    Deadline(SystemTime),  // 截止时间类型
}

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,      // 任务名称（标签）
    pub task_type: TaskType,
    pub is_running: bool,  // 是否正在运行
    pub start_time: Option<SystemTime>, // 开始时间
    pub remaining: Duration, // 剩余时间
    pub pinned: bool,      // 是否固定
}

impl Task {
    pub fn new(name: String, task_type: TaskType) -> Self {
        let remaining = match &task_type {
            TaskType::Duration(d) => *d,
            TaskType::Deadline(t) => t.duration_since(SystemTime::now()).unwrap_or(Duration::ZERO),
        };

        Self {
            name,
            task_type,
            is_running: false,
            start_time: None,
            remaining,
            pinned: false,
        }
    }

    pub fn start(&mut self) {
        if !self.is_running {
            self.is_running = true;
            self.start_time = Some(SystemTime::now());
        }
    }

    pub fn pause(&mut self) {
        if self.is_running {
            self.is_running = false;
            if let Some(start) = self.start_time {
                if let Ok(elapsed) = start.elapsed() {
                    self.remaining = self.remaining.saturating_sub(elapsed);
                }
            }
            self.start_time = None;
        }
    }

    pub fn reset(&mut self) {
        self.is_running = false;
        self.start_time = None;
        self.remaining = match &self.task_type {
            TaskType::Duration(d) => *d,
            TaskType::Deadline(t) => t.duration_since(SystemTime::now()).unwrap_or(Duration::ZERO),
        };
    }

    pub fn get_remaining_time(&self) -> Duration {
        if !self.is_running {
            return self.remaining;
        }

        if let Some(start) = self.start_time {
            if let Ok(elapsed) = start.elapsed() {
                return self.remaining.saturating_sub(elapsed);
            }
        }
        self.remaining
    }
} 