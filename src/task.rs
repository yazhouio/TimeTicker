use std::time::{Duration, SystemTime};
use crate::error::{Result, system_time_to_duration, SystemTimeSnafu}; // Import Result and helpers
use snafu::{OptionExt, ResultExt}; // For .context on Option and Result

#[derive(Debug, Clone)]
pub enum TaskType {
    Duration(Duration),   // 时间段类型
    Deadline(SystemTime), // 截止时间类型
}

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String, // 任务名称（标签）
    pub task_type: TaskType,
    pub is_running: bool,               // 是否正在运行
    pub start_time: Option<SystemTime>, // 开始时间
    pub remaining: Duration,            // 剩余时间
    pub pinned: bool,                   // 是否固定
}

impl Task {
    // Changed to return Result to handle potential errors from duration_since
    pub fn new(name: String, task_type: TaskType) -> Result<Self> {
        let remaining = match &task_type {
            TaskType::Duration(d) => *d,
            TaskType::Deadline(t) => {
                system_time_to_duration(*t)? // Use helper
                    .saturating_sub(system_time_to_duration(SystemTime::now())?) // Use helper
            }
        };

        Ok(Self {
            name,
            task_type,
            is_running: false,
            start_time: None,
            remaining,
            pinned: false,
        })
    }

    pub fn start(&mut self) {
        if !self.is_running {
            self.is_running = true;
            self.start_time = Some(SystemTime::now());
        }
    }

    // Changed to return Result to handle potential errors from start.elapsed()
    pub fn pause(&mut self) -> Result<()> {
        if self.is_running {
            self.is_running = false;
            if let Some(start) = self.start_time {
                let elapsed = start.elapsed().context(SystemTimeSnafu)?; // Handle error
                self.remaining = self.remaining.saturating_sub(elapsed);
            }
            self.start_time = None;
        }
        Ok(())
    }

    // Changed to return Result to handle potential errors from duration_since
    pub fn reset(&mut self) -> Result<()> {
        self.is_running = false;
        self.start_time = None;
        self.remaining = match &self.task_type {
            TaskType::Duration(d) => *d,
            TaskType::Deadline(t) => {
                system_time_to_duration(*t)? // Use helper
                    .saturating_sub(system_time_to_duration(SystemTime::now())?) // Use helper
            }
        };
        Ok(())
    }

    // Changed to return Result to handle potential errors
    pub fn get_remaining_time(&self) -> Result<Duration> {
        match &self.task_type {
            TaskType::Duration(_) => {
                if !self.is_running {
                    return Ok(self.remaining);
                }

                if let Some(start) = self.start_time {
                    let elapsed = start.elapsed().context(SystemTimeSnafu)?; // Handle error
                    return Ok(self.remaining.saturating_sub(elapsed));
                }
                Ok(self.remaining)
            }
            TaskType::Deadline(deadline) => {
                Ok(system_time_to_duration(*deadline)? // Use helper
                    .saturating_sub(system_time_to_duration(SystemTime::now())?)) // Use helper
            }
        }
    }
}
