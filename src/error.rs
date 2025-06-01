use snafu::{Snafu, Backtrace, ErrorCompat, ResultExt}; // Import ResultExt
use std::path::PathBuf;
// crate::parser::ParseError is no longer used as parser.rs uses this Error enum directly.
use tray_icon::Error as TrayIconError;
use tray_icon::BadIcon; // Import BadIcon
use image::ImageError;
use winit::error::{EventLoopError as WinitEventLoopError, OsError as WinitOsError}; // Corrected winit error imports
use winit::event_loop::EventLoopClosed as WinitEventLoopClosedError; // For EventLoopSend
use std::num::ParseIntError;
use regex::Error as RegexError;
use muda::Error as MudaError; // For MenuAppend
use chrono::ParseError as ChronoParseError;


#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    // General Errors
    #[snafu(display("I/O error for path '{}': {}", path.display(), source))]
    Io { path: PathBuf, source: std::io::Error, backtrace: Backtrace },

    // Task related errors (from task.rs or main.rs involving tasks)
    #[snafu(display("Task not found at index: {}", index))]
    TaskNotFound { index: usize, backtrace: Backtrace },
    #[snafu(display("Failed to acquire lock on tasks"))]
    TaskLock { backtrace: Backtrace },
    #[snafu(display("SystemTime error: {}", source))]
    SystemTimeError { source: std::time::SystemTimeError, backtrace: Backtrace }, // Added source

    // Tray Icon and Menu Errors (from main.rs)
    #[snafu(display("Tray icon build error: {}", source))]
    TrayIconBuild { source: TrayIconError, backtrace: Backtrace },
    #[snafu(display("Image error loading icon: {}", source))]
    Image { source: ImageError, backtrace: Backtrace }, // For load_icon
    #[snafu(display("Icon conversion error: {}", source))]
    IconConversion { source: BadIcon, backtrace: Backtrace },
    #[snafu(display("Failed to append menu item '{}': {}", item_name, source))]
    MenuAppend { source: MudaError, item_name: String, backtrace: Backtrace }, // Corrected source to MudaError
    #[snafu(display("Failed to update tray icon (operation: {}): {}", operation, source))]
    TrayIconUpdate { operation: String, source: TrayIconError, backtrace: Backtrace },
    #[snafu(display("Invalid action string format: '{}', expected prefix: '{}'", action_string, expected_prefix))]
    InvalidActionFormat { action_string: String, expected_prefix: String, backtrace: Backtrace },
    #[snafu(display("Failed to parse index from action string '{}': {}", action_string, source))]
    ParseActionIndex { source: ParseIntError, action_string: String, backtrace: Backtrace },

    // Event Loop and Windowing Errors (from main.rs)
    #[snafu(display("Failed to create event loop: {}", source))]
    EventLoopCreation { source: WinitEventLoopError, backtrace: Backtrace },
    #[snafu(display("Failed to send event to event loop: {}", source))]
    EventLoopSend { source: WinitEventLoopClosedError<crate::UserEvent>, backtrace: Backtrace }, // Corrected source type
    #[snafu(display("Failed to create window: {}", source))]
    WindowCreation { source: WinitOsError, backtrace: Backtrace },

    // macOS Specific Errors (from main.rs)
    #[snafu(display("Failed to get main thread marker for macOS operation"))]
    MainThreadMarker { backtrace: Backtrace },
    #[snafu(display("Failed to canonicalize path '{}': {}", path.display(), source))]
    CanonicalizePath { path: PathBuf, source: std::io::Error, backtrace: Backtrace },
    #[snafu(display("Failed to get macOS main run loop"))]
    MacOsMainRunLoopUnavailable { backtrace: Backtrace },
    #[snafu(display("Failed to execute AppleScript: {}", source))]
    AppleScriptExecution { source: std::io::Error, backtrace: Backtrace },
    #[snafu(display("AppleScript returned non-UTF8 output"))]
    AppleScriptOutput { backtrace: Backtrace }, // Could wrap FromUtf8Error
    #[snafu(display("Failed to parse AppleScript output"))]
    AppleScriptParse { backtrace: Backtrace }, // Could be more specific

    // Parser Errors (from parser.rs - to be used by parser.rs)
    // This replaces the old `Parse { source: ParseError, ... }` if parser.rs uses this Error enum.
    // If parser.rs has its own error enum that it converts *to* this, then the original Parse variant might be okay.
    // For now, assuming parser.rs will use these directly or via `From`.
    #[snafu(display("Regex compilation failed: {}", source))]
    RegexCompile { source: RegexError, backtrace: Backtrace },
    #[snafu(display("Invalid input format: {}", msg))]
    InvalidInputFormat { msg: String, backtrace: Backtrace },
    #[snafu(display("Missing time input: {}", msg))]
    MissingTimeInput { msg: String, backtrace: Backtrace },
    #[snafu(display("Failed to parse time string: {}", source))]
    ChronoParse { source: ChronoParseError, backtrace: Backtrace },
    #[snafu(display("Timezone conversion failed: {}", msg))]
    TimezoneConversion { msg: String, backtrace: Backtrace },
    #[snafu(display("Failed to parse number from input: {}", source))]
    ParseNumber { source: ParseIntError, backtrace: Backtrace },
    #[snafu(display("Invalid duration unit: '{}'", unit))]
    InvalidDurationUnit { unit: String, backtrace: Backtrace },
    #[snafu(display("Duration cannot be zero"))]
    ZeroDuration { backtrace: Backtrace },
    // ParserErrorWrapper is removed as parser.rs now uses variants from this Error enum directly.
}

// The SystemTimeSnafu struct is removed. Snafu will auto-generate SystemTimeErrorSnafu.

pub type Result<T, E = Error> = std::result::Result<T, E>;

// Helper for unwraps related to SystemTime
pub fn system_time_to_duration(system_time: std::time::SystemTime) -> Result<std::time::Duration> {
    system_time.duration_since(std::time::UNIX_EPOCH).context(SystemTimeSnafu)
}