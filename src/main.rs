#![allow(unused)]

mod error;
mod parser;
mod task;

use std::{
    collections::HashMap,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use image::{ImageBuffer, Rgba, RgbaImage};
#[cfg(target_os = "macos")]
use objc2::{ClassType, msg_send_id};
// macOS 特定导入，用于 Dock 控制
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApp, NSApplication, NSApplicationActivationPolicy, NSImage};
#[cfg(target_os = "macos")]
use objc2_foundation::{MainThreadMarker, NSData, NSString};
use parser::parse_time_input;
use snafu::{Backtrace, ResultExt, prelude::*};
use task::{Task, TaskType};
use tracing::{debug, error, info, trace, warn};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconEventReceiver,
    menu::{Menu, MenuEvent as TrayMenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
};
use winit::{
    application::ApplicationHandler,
    event::Event,
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    window::Window,
};

use crate::error::{
    CanonicalizePathSnafu, Error, EventLoopCreationSnafu, EventLoopSendSnafu, IconConversionSnafu, ImageSnafu,
    InvalidActionFormatSnafu, IoSnafu, MacOsMainRunLoopUnavailableSnafu, MainThreadMarkerSnafu, MenuAppendSnafu,
    ParseActionIndexSnafu, /* ParserErrorWrapperSnafu was correctly removed. SystemTimeSnafu was correctly changed
                            * to SystemTimeErrorSnafu. */
    Result, TaskLockSnafu, TrayIconBuildSnafu, TrayIconUpdateSnafu, WindowCreationSnafu,
};

#[derive(Debug)]
enum UserEvent {
    TrayIconEvent(tray_icon::TrayIconEvent),
    MenuEvent(TrayMenuEvent),
    UpdateTimer,
    StartTask(usize),
    PauseTask(usize),
    ResetTask(usize),
    DeleteTask(usize),
}

struct Application {
    tray_icon: Option<TrayIcon>,
    tasks: Arc<Mutex<Vec<Task>>>,
    menu_ids: HashMap<MenuId, String>,              // 菜单ID到动作的映射
    menu_items: HashMap<usize, Submenu>,            // 任务索引到子菜单的映射，用于更新文本
    control_items: HashMap<usize, MenuItem>,        // 任务索引到控制按钮的映射
    pinned_tray_icons: HashMap<usize, TrayIcon>,    // 固定任务的独立托盘图标
    pinned_menu_items: HashMap<usize, MenuItem>,    // 固定托盘菜单中的时间显示项
    pinned_control_items: HashMap<usize, MenuItem>, // 固定托盘菜单中的控制按钮
}

impl Application {
    fn new() -> Self {
        // 创建一些测试任务
        let test_tasks_results = vec![
            Task::new(
                "工作1".to_string(),
                TaskType::Deadline(SystemTime::now() + Duration::from_secs(3600)),
            ),
            Task::new("学习1".to_string(), TaskType::Duration(Duration::from_secs(30 * 60))),
            Task::new(
                "工作2".to_string(),
                TaskType::Deadline(SystemTime::now() + Duration::from_secs(7200)),
            ),
            Task::new("学习2".to_string(), TaskType::Duration(Duration::from_secs(15 * 60))),
            Task::new(
                "工作3".to_string(),
                TaskType::Deadline(SystemTime::now() + Duration::from_secs(10800)),
            ),
            Task::new("学习3".to_string(), TaskType::Duration(Duration::from_secs(45 * 60))),
        ];

        let test_tasks: Vec<Task> = test_tasks_results
            .into_iter()
            .filter_map(|task_result| match task_result {
                Ok(task) => Some(task),
                Err(e) => {
                    error!("Failed to create initial task: {}", e);
                    None
                }
            })
            .collect();

        Self {
            tray_icon: None,
            tasks: Arc::new(Mutex::new(test_tasks)),
            menu_ids: HashMap::new(),
            menu_items: HashMap::new(),
            control_items: HashMap::new(),
            pinned_tray_icons: HashMap::new(),
            pinned_menu_items: HashMap::new(),
            pinned_control_items: HashMap::new(),
        }
    }

    fn new_tray_icon(&mut self) -> Result<TrayIcon> {
        let path = std::path::Path::new("./assets/logo.png");
        let icon = load_icon(path)?;

        let menu = self.build_menu()?;

        TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Time Ticker")
            .with_icon(icon)
            .build()
            .context(TrayIconBuildSnafu)
    }

    fn build_menu(&mut self) -> Result<Menu> {
        let menu = Menu::new();

        // 保存固定托盘菜单的ID，避免被清除
        let pinned_menu_ids: Vec<(MenuId, String)> = self
            .menu_ids
            .iter()
            .filter(|(_, action)| action.starts_with("pinned_") || action.starts_with("unpin_"))
            .map(|(id, action)| (id.clone(), action.clone()))
            .collect();

        self.menu_ids.clear(); // 清除旧的菜单ID映射
        self.menu_items.clear(); // 清除旧的菜单项映射
        self.control_items.clear(); // 清除旧的控制项映射

        // 恢复固定托盘菜单的ID
        for (id, action) in pinned_menu_ids {
            self.menu_ids.insert(id, action);
        }

        // 添加任务菜单项
        {
            let tasks = self.tasks.lock().map_err(|_| error::TaskLockSnafu.build())?;
            for (i, task) in tasks.iter().enumerate() {
                // 显示剩余时间的子菜单
                let remaining_time = task.get_remaining_time()?;
                let time_str = format_remaining_time(remaining_time);
                let task_submenu = Submenu::new(format!("{}#{}", time_str, task.name), true);
                self.menu_items.insert(i, task_submenu.clone()); // 存储子菜单引用

                // 根据任务类型添加不同的控制选项
                match task.task_type {
                    TaskType::Duration(_) => {
                        // 开始/暂停
                        let start_pause = MenuItem::new(if task.is_running { "暂停" } else { "开始" }, true, None);
                        let start_pause_id = start_pause.id().clone();
                        self.menu_ids.insert(start_pause_id, format!("toggle_{i}"));
                        self.control_items.insert(i, start_pause.clone()); // 存储控制项引用
                        task_submenu.append(&start_pause).context(MenuAppendSnafu {
                            item_name: format!("start_pause_task_{}", i),
                        })?;

                        // 重置
                        let reset = MenuItem::new("重置", true, None);
                        let reset_id = reset.id().clone();
                        self.menu_ids.insert(reset_id, format!("reset_{i}"));
                        task_submenu.append(&reset).context(MenuAppendSnafu {
                            item_name: format!("reset_task_{}", i),
                        })?;
                    }
                    TaskType::Deadline(_) => {
                        // 截止时间类型任务不需要开始/暂停/重置
                    }
                }

                // 添加分隔线
                task_submenu
                    .append(&PredefinedMenuItem::separator())
                    .context(MenuAppendSnafu {
                        item_name: format!("separator_after_controls_task_{}", i),
                    })?;

                // 新增任务
                let new_task_item = MenuItem::new("新增", true, None);
                let new_task_id = new_task_item.id().clone();
                self.menu_ids.insert(new_task_id, "new_task".to_string());
                task_submenu.append(&new_task_item).context(MenuAppendSnafu {
                    item_name: format!("new_sub_task_{}", i),
                })?;

                // 编辑
                let edit = MenuItem::new("编辑", true, None);
                let edit_id = edit.id().clone();
                self.menu_ids.insert(edit_id, format!("edit_{i}"));
                task_submenu.append(&edit).context(MenuAppendSnafu {
                    item_name: format!("edit_task_{}", i),
                })?;

                // 删除
                let delete = MenuItem::new("删除", true, None);
                let delete_id = delete.id().clone();
                self.menu_ids.insert(delete_id, format!("delete_{i}"));
                task_submenu.append(&delete).context(MenuAppendSnafu {
                    item_name: format!("delete_task_{}", i),
                })?;

                // 固定/取消固定
                let pin = MenuItem::new(if task.pinned { "取消固定" } else { "固定" }, true, None);
                let pin_id = pin.id().clone();
                self.menu_ids.insert(pin_id, format!("pin_{i}"));
                task_submenu.append(&pin).context(MenuAppendSnafu {
                    item_name: format!("pin_task_{}", i),
                })?;

                // 将子菜单添加到主菜单
                menu.append(&task_submenu).context(MenuAppendSnafu {
                    item_name: format!("task_submenu_{}", i),
                })?;
            }
        }

        // 添加分隔线
        menu.append(&PredefinedMenuItem::separator()).context(MenuAppendSnafu {
            item_name: "separator_after_tasks".to_string(),
        })?;

        // 添加新建任务选项
        let new_task_main = MenuItem::new("新建任务", true, None);
        let new_task_main_id = new_task_main.id().clone();
        self.menu_ids.insert(new_task_main_id, "new_task".to_string());
        menu.append(&new_task_main).context(MenuAppendSnafu {
            item_name: "new_task_main".to_string(),
        })?;

        // 添加设置选项
        let settings_submenu = Submenu::new("⚙️ 设置", true);

        // Dock 设置
        let dock_submenu = Submenu::new("🖥️ Dock 设置", true);

        let show_dock = MenuItem::new("显示在 Dock 中", true, None);
        let show_dock_id = show_dock.id().clone();
        self.menu_ids.insert(show_dock_id, "dock_show".to_string());
        dock_submenu.append(&show_dock).context(MenuAppendSnafu {
            item_name: "dock_show".to_string(),
        })?;

        let hide_dock = MenuItem::new("隐藏 Dock 图标", true, None);
        let hide_dock_id = hide_dock.id().clone();
        self.menu_ids.insert(hide_dock_id, "dock_hide".to_string());
        dock_submenu.append(&hide_dock).context(MenuAppendSnafu {
            item_name: "dock_hide".to_string(),
        })?;

        // 添加分隔线
        dock_submenu
            .append(&PredefinedMenuItem::separator())
            .context(MenuAppendSnafu {
                item_name: "dock_separator".to_string(),
            })?;

        // 添加测试图标设置
        let test_icon = MenuItem::new("🔄 重新设置 dock.png", true, None);
        let test_icon_id = test_icon.id().clone();
        self.menu_ids.insert(test_icon_id, "dock_test_icon".to_string());
        dock_submenu.append(&test_icon).context(MenuAppendSnafu {
            item_name: "dock_test_icon".to_string(),
        })?;

        settings_submenu.append(&dock_submenu).context(MenuAppendSnafu {
            item_name: "dock_submenu".to_string(),
        })?;
        menu.append(&settings_submenu).context(MenuAppendSnafu {
            item_name: "settings_submenu".to_string(),
        })?;

        // 添加分隔线
        menu.append(&PredefinedMenuItem::separator()).context(MenuAppendSnafu {
            item_name: "separator_before_quit".to_string(),
        })?;

        // 添加退出选项
        let quit = MenuItem::new("退出", true, None);
        let quit_id = quit.id().clone();
        self.menu_ids.insert(quit_id, "quit".to_string());
        menu.append(&quit).context(MenuAppendSnafu {
            item_name: "quit".to_string(),
        })?;

        Ok(menu)
    }

    fn update_tray_icon(&self) -> Result<()> {
        if let Some(tray_icon) = &self.tray_icon {
            let tasks = self.tasks.lock().map_err(|_| TaskLockSnafu.build())?; // Use TaskLockSnafu directly
            let mut tooltip = String::new();

            // 更新tooltip和菜单项文本
            for (i, task) in tasks.iter().enumerate() {
                let remaining = task.get_remaining_time()?;
                let time_str = format_remaining_time(remaining);
                tooltip.push_str(&format!("{}#{}\n", time_str, task.name));

                // 更新菜单项文本（不会关闭菜单）
                if let Some(menu_item) = self.menu_items.get(&i) {
                    menu_item.set_text(format!("{}#{}", time_str, task.name));
                }

                // 更新控制按钮文本
                if let Some(control_item) = self.control_items.get(&i)
                    && let TaskType::Duration(_) = task.task_type
                {
                    control_item.set_text(if task.is_running { "暂停" } else { "开始" });
                }
            }

            tray_icon.set_tooltip(Some(&tooltip)).context(TrayIconUpdateSnafu {
                operation: "set_tooltip".to_string(),
            })?;
            drop(tasks);
        }

        // 更新所有固定的托盘图标
        let pinned_indices: Vec<usize> = self.pinned_tray_icons.keys().cloned().collect();
        for index in pinned_indices {
            if let Err(e) = self.update_pinned_tray_icon(index) {
                error!("Failed to update pinned tray icon for task {}: {}", index, e);
            }
        }
        Ok(())
    }

    fn refresh_menu(&mut self) -> Result<()> {
        let new_menu = self.build_menu()?;
        if let Some(tray_icon) = &self.tray_icon {
            tray_icon.set_menu(Some(Box::new(new_menu))); // Use TrayIconUpdateSnafu directly
        }
        Ok(())
    }

    fn create_pinned_tray_icon(&mut self, task_index: usize) -> Result<()> {
        let path = std::path::Path::new("./assets/logo.png");
        let icon_res = load_icon(path); // Keep as Result for now

        // 先获取任务信息，然后释放锁
        let (task_name, task_type, is_running, remaining_time_res) = {
            let tasks = self.tasks.lock().map_err(|_| error::TaskLockSnafu.build())?;
            if let Some(task) = tasks.get(task_index) {
                (
                    task.name.clone(),
                    task.task_type.clone(),
                    task.is_running,
                    task.get_remaining_time(),
                )
            } else {
                // This case should ideally be an error, but to match original logic, we return
                // Ok. Consider changing to `Err(Error::TaskNotFound { index:
                // task_index, ... })`
                return Ok(());
            }
        };
        let remaining_time = remaining_time_res?; // Handle Result for remaining_time

        // 现在可以安全地调用 build_pinned_task_menu
        let menu = self.build_pinned_task_menu(task_index, &task_name, &task_type, is_running, remaining_time)?;

        // 使用时间文本作为标题，格式：MM:SS
        let time_str = format_remaining_time(remaining_time); // remaining_time is already Duration here
        let parts: Vec<&str> = time_str.split(':').collect();
        let time_title = if parts.len() >= 3 {
            format!("{}:{}", parts[1], parts[2]) // 显示 MM:SS
        } else {
            "00:00".to_string()
        };

        let final_icon = icon_res?; // Handle icon Result here

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(format!("{}#{}", format_remaining_time(remaining_time), task_name)) // remaining_time is Duration
            .with_icon(final_icon)
            .with_title(&time_title)
            .build()
            .context(TrayIconBuildSnafu)?; // Use TrayIconBuildSnafu directly

        self.pinned_tray_icons.insert(task_index, tray_icon);
        Ok(())
    }

    fn build_pinned_task_menu(
        &mut self,
        task_index: usize,
        task_name: &str,
        task_type: &TaskType,
        is_running: bool,
        remaining_time: Duration,
    ) -> Result<Menu> {
        let menu = Menu::new();

        // 显示任务时间（正确显示当前剩余时间）
        let time_str = format_remaining_time(remaining_time);
        let time_item = MenuItem::new(format!("{time_str}#{task_name}"), false, None);
        self.pinned_menu_items.insert(task_index, time_item.clone()); // 保存引用以便更新
        menu.append(&time_item).context(MenuAppendSnafu {
            item_name: format!("pinned_time_item_task_{}", task_index),
        })?;

        // 添加分隔线
        menu.append(&PredefinedMenuItem::separator()).context(MenuAppendSnafu {
            item_name: format!("pinned_separator1_task_{}", task_index),
        })?;

        // 根据任务类型添加控制选项
        match task_type {
            TaskType::Duration(_) => {
                // 开始/暂停
                let start_pause = MenuItem::new(if is_running { "暂停" } else { "开始" }, true, None);
                let start_pause_id = start_pause.id().clone();
                self.menu_ids
                    .insert(start_pause_id, format!("pinned_toggle_{task_index}"));
                self.pinned_control_items.insert(task_index, start_pause.clone()); // 保存引用以便更新
                menu.append(&start_pause).context(MenuAppendSnafu {
                    item_name: format!("pinned_toggle_task_{}", task_index),
                })?;

                // 重置
                let reset = MenuItem::new("重置", true, None);
                let reset_id = reset.id().clone();
                self.menu_ids.insert(reset_id, format!("pinned_reset_{task_index}"));
                menu.append(&reset).context(MenuAppendSnafu {
                    item_name: format!("pinned_reset_task_{}", task_index),
                })?;
            }
            TaskType::Deadline(_) => {
                // 截止时间类型任务不需要开始/暂停/重置
            }
        }

        // 添加分隔线
        menu.append(&PredefinedMenuItem::separator()).context(MenuAppendSnafu {
            item_name: format!("pinned_separator2_task_{}", task_index),
        })?;

        // 取消固定
        let unpin = MenuItem::new("取消固定", true, None);
        let unpin_id = unpin.id().clone();
        self.menu_ids.insert(unpin_id, format!("unpin_{task_index}"));
        menu.append(&unpin).context(MenuAppendSnafu {
            item_name: format!("unpin_task_{}", task_index),
        })?;

        Ok(menu)
    }

    fn remove_pinned_tray_icon(&mut self, task_index: usize) {
        self.pinned_tray_icons.remove(&task_index);
        self.pinned_menu_items.remove(&task_index);
        self.pinned_control_items.remove(&task_index);
    }

    fn update_pinned_tray_icon(&self, task_index: usize) -> Result<()> {
        // 先获取任务信息
        let (task_name, task_type, is_running, remaining_time) = {
            let tasks = self.tasks.lock().map_err(|_| error::TaskLockSnafu.build())?;
            if let Some(task) = tasks.get(task_index) {
                (
                    task.name.clone(),
                    task.task_type.clone(),
                    task.is_running,
                    task.get_remaining_time(),
                )
            } else {
                // Consider returning an error here if task not found
                return Ok(()); // Matching original behavior
            }
        };
        let remaining_time = remaining_time?; // Handle Result from get_remaining_time

        // 更新托盘图标
        if let Some(tray_icon) = self.pinned_tray_icons.get(&task_index) {
            let time_str = format_remaining_time(remaining_time); // Handle Result from get_remaining_time
            let tooltip = format!("{time_str}#{task_name}");

            // 使用文本标题显示时间，格式：MM:SS
            let parts: Vec<&str> = time_str.split(':').collect();
            let time_title = if parts.len() >= 3 {
                format!("{}:{}", parts[1], parts[2]) // 显示 MM:SS
            } else {
                "00:00".to_string()
            };

            tray_icon.set_title(Some(&time_title));
            tray_icon.set_tooltip(Some(&tooltip)).context(TrayIconUpdateSnafu {
                operation: format!("set_tooltip_pinned_task_{}", task_index),
            })?;
        }

        // 更新固定菜单中的时间显示项（不重新构建菜单，避免菜单消失）
        if let Some(menu_item) = self.pinned_menu_items.get(&task_index) {
            let time_str = format_remaining_time(remaining_time); // Handle Result from get_remaining_time
            menu_item.set_text(format!("{time_str}#{task_name}"));
        }

        // 更新固定菜单中的控制按钮文本
        if let Some(control_item) = self.pinned_control_items.get(&task_index)
            && let TaskType::Duration(_) = task_type
        {
            control_item.set_text(if is_running { "暂停" } else { "开始" });
        }
        Ok(())
    }

    fn create_time_icon(&self, time_str: &str) -> Result<Icon> {
        // 直接使用简化版本，绘制数字时间
        self.create_digital_time_icon(time_str)
    }

    fn create_digital_time_icon(&self, time_str: &str) -> Result<Icon> {
        // 创建一个32x32的图像
        let width = 32u32;
        let height = 32u32;
        let mut img: RgbaImage = ImageBuffer::new(width, height);

        // 填充背景色（深色背景）
        for pixel in img.pixels_mut() {
            *pixel = Rgba([45, 45, 45, 255]); // 深灰色背景
        }

        // 解析时间字符串 (HH:MM:SS)
        let parts: Vec<&str> = time_str.split(':').collect();
        if parts.len() >= 3 {
            let minutes = parts[1];
            let seconds = parts[2];

            // 绘制时间数字（更大的字体，更好的间距）
            let display_time = format!("{minutes}:{seconds}");
            self.draw_large_text(&mut img, &display_time, 1, 10);
        } else {
            // 如果解析失败，显示时钟图标
            self.draw_clock_icon(&mut img);
        }

        // 转换为Icon
        let rgba_data = img.into_raw();
        Icon::from_rgba(rgba_data, width, height).context(IconConversionSnafu) // Use IconConversionSnafu directly
    }

    fn draw_large_text(&self, img: &mut RgbaImage, text: &str, x: u32, y: u32) {
        // 更大的像素字体绘制，适合托盘图标
        let white = Rgba([255, 255, 255, 255]);

        let mut current_x = x;
        for ch in text.chars() {
            match ch {
                '0' => self.draw_large_digit_0(img, current_x, y, white),
                '1' => self.draw_large_digit_1(img, current_x, y, white),
                '2' => self.draw_large_digit_2(img, current_x, y, white),
                '3' => self.draw_large_digit_3(img, current_x, y, white),
                '4' => self.draw_large_digit_4(img, current_x, y, white),
                '5' => self.draw_large_digit_5(img, current_x, y, white),
                '6' => self.draw_large_digit_6(img, current_x, y, white),
                '7' => self.draw_large_digit_7(img, current_x, y, white),
                '8' => self.draw_large_digit_8(img, current_x, y, white),
                '9' => self.draw_large_digit_9(img, current_x, y, white),
                ':' => self.draw_large_colon(img, current_x, y, white),
                _ => {}
            }
            current_x += if ch == ':' { 3 } else { 6 }; // 更大的间距
        }
    }

    fn draw_simple_text(&self, img: &mut RgbaImage, text: &str, x: u32, y: u32) {
        // 简单的像素字体绘制
        let white = Rgba([255, 255, 255, 255]);

        let mut current_x = x;
        for ch in text.chars() {
            match ch {
                '0' => self.draw_digit_0(img, current_x, y, white),
                '1' => self.draw_digit_1(img, current_x, y, white),
                '2' => self.draw_digit_2(img, current_x, y, white),
                '3' => self.draw_digit_3(img, current_x, y, white),
                '4' => self.draw_digit_4(img, current_x, y, white),
                '5' => self.draw_digit_5(img, current_x, y, white),
                '6' => self.draw_digit_6(img, current_x, y, white),
                '7' => self.draw_digit_7(img, current_x, y, white),
                '8' => self.draw_digit_8(img, current_x, y, white),
                '9' => self.draw_digit_9(img, current_x, y, white),
                ':' => self.draw_colon(img, current_x, y, white),
                _ => {}
            }
            current_x += if ch == ':' { 2 } else { 4 };
        }
    }

    fn draw_clock_icon(&self, img: &mut RgbaImage) {
        let white = Rgba([255, 255, 255, 255]);

        // 绘制圆形边框
        for y in 8..24 {
            for x in 8..24 {
                let dx = (x as i32 - 16).abs();
                let dy = (y as i32 - 16).abs();
                let distance = ((dx * dx + dy * dy) as f32).sqrt();

                if (6.0..=8.0).contains(&distance) {
                    img.put_pixel(x, y, white);
                }
            }
        }

        // 绘制时钟指针
        // 短针（小时）
        for i in 0..4 {
            img.put_pixel(16, 16 - i, white);
        }
        // 长针（分钟）
        for i in 0..6 {
            img.put_pixel(16 + i, 16, white);
        }
    }

    // 简单的3x5像素字体
    fn draw_digit_0(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 1, 1], [1, 0, 1], [1, 0, 1], [1, 0, 1], [1, 1, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_1(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[0, 1, 0], [1, 1, 0], [0, 1, 0], [0, 1, 0], [1, 1, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_2(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 1, 1], [0, 0, 1], [1, 1, 1], [1, 0, 0], [1, 1, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_3(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 1, 1], [0, 0, 1], [1, 1, 1], [0, 0, 1], [1, 1, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_4(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 0, 1], [1, 0, 1], [1, 1, 1], [0, 0, 1], [0, 0, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_5(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 1, 1], [1, 0, 0], [1, 1, 1], [0, 0, 1], [1, 1, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_6(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 1, 1], [1, 0, 0], [1, 1, 1], [1, 0, 1], [1, 1, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_7(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 1, 1], [0, 0, 1], [0, 0, 1], [0, 0, 1], [0, 0, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_8(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 1, 1], [1, 0, 1], [1, 1, 1], [1, 0, 1], [1, 1, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_digit_9(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [[1, 1, 1], [1, 0, 1], [1, 1, 1], [0, 0, 1], [1, 1, 1]];
        self.draw_pattern(img, x, y, &pattern, color);
    }

    fn draw_colon(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        if x + 1 < img.width() && y + 4 < img.height() {
            img.put_pixel(x, y + 1, color);
            img.put_pixel(x, y + 3, color);
        }
    }

    fn draw_pattern(&self, img: &mut RgbaImage, x: u32, y: u32, pattern: &[[u8; 3]; 5], color: Rgba<u8>) {
        for (row, line) in pattern.iter().enumerate() {
            for (col, &pixel) in line.iter().enumerate() {
                if pixel == 1 {
                    let px = x + col as u32;
                    let py = y + row as u32;
                    if px < img.width() && py < img.height() {
                        img.put_pixel(px, py, color);
                    }
                }
            }
        }
    }

    // 大字体绘制方法 (5x7 像素)
    fn draw_large_pattern(&self, img: &mut RgbaImage, x: u32, y: u32, pattern: &[[u8; 5]; 7], color: Rgba<u8>) {
        for (row, line) in pattern.iter().enumerate() {
            for (col, &pixel) in line.iter().enumerate() {
                if pixel == 1 {
                    let px = x + col as u32;
                    let py = y + row as u32;
                    if px < img.width() && py < img.height() {
                        img.put_pixel(px, py, color);
                    }
                }
            }
        }
    }

    fn draw_large_digit_0(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_1(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [0, 0, 1, 0, 0],
            [0, 1, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [1, 1, 1, 1, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_2(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 1, 1, 1, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 1, 1, 1, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_3(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 1, 1, 1, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_4(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_5(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 1, 1, 1, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_6(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_7(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 1, 1, 1, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_8(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_digit_9(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        let pattern = [
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
        ];
        self.draw_large_pattern(img, x, y, &pattern, color);
    }

    fn draw_large_colon(&self, img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
        if x + 1 < img.width() && y + 6 < img.height() {
            img.put_pixel(x, y + 2, color);
            img.put_pixel(x, y + 4, color);
        }
    }

    #[allow(clippy::cognitive_complexity)]
    fn handle_menu_event(&mut self, event: TrayMenuEvent) {
        let menu_id = event.id;

        debug!("菜单事件触发，ID: {:?}", menu_id);

        if let Some(action) = self.menu_ids.get(&menu_id).cloned() {
            debug!("找到对应动作: {}", action);
            if action == "quit" {
                std::process::exit(0);
            } else if action == "dock_show" {
                info!("🖥️ 显示 Dock 图标");
                #[cfg(target_os = "macos")]
                {
                    if let Err(e) = set_dock_visibility(true) {
                        error!("Failed to show dock: {}", e);
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    // For non-macOS, set_dock_visibility itself will warn.
                    // We can call it to maintain consistent behavior if it has non-macOS logic,
                    // or just warn here if it's purely a no-op that returns Ok(()).
                    if let Err(e) = set_dock_visibility(true) {
                        // Assuming it might do something or log
                        error!("set_dock_visibility(true) failed on non-macOS (unexpected): {}", e);
                    }
                    warn!("Dock visibility control is primarily a macOS feature.");
                }
            } else if action == "dock_hide" {
                info!("🖥️ 隐藏 Dock 图标");
                #[cfg(target_os = "macos")]
                {
                    if let Err(e) = set_dock_visibility(false) {
                        error!("Failed to hide dock: {}", e);
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    if let Err(e) = set_dock_visibility(false) {
                        error!("set_dock_visibility(false) failed on non-macOS (unexpected): {}", e);
                    }
                    warn!("Dock visibility control is primarily a macOS feature.");
                }
            } else if action == "dock_test_icon" {
                info!("🔄 手动重新设置 Dock 图标");
                #[cfg(target_os = "macos")]
                {
                    if let Err(e) = set_dock_icon() {
                        error!("Failed to set dock icon: {}", e);
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    warn!("Dock icon control is only available on macOS.");
                }
            } else if action == "new_task" {
                // 实现新建任务功能
                self.handle_new_task();
            } else if action.starts_with("task_") {
                // 处理任务点击
                println!("点击了任务");
            } else if action.starts_with("toggle_") {
                match action
                    .strip_prefix("toggle_")
                    .ok_or_else(|| {
                        InvalidActionFormatSnafu {
                            action_string: action.clone(),
                            expected_prefix: "toggle_",
                        }
                        .build()
                    })
                    .and_then(|s| {
                        s.parse::<usize>().context(ParseActionIndexSnafu {
                            action_string: s.to_string(),
                        })
                    }) {
                    Ok(index) => {
                        if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                            if let Some(task) = tasks.get_mut(index) {
                                if task.is_running {
                                    if let Err(e) = task.pause() {
                                        error!("Failed to pause task {}: {}", task.name, e);
                                    } else {
                                        info!("⏸️ 任务 '{}' 已暂停", task.name);
                                    }
                                } else {
                                    task.start();
                                    info!("▶️ 任务 '{}' 已开始", task.name);
                                }
                            } else {
                                error!("Task not found at index {} for toggle", index);
                            }
                        } else {
                            error!("Failed to lock tasks for toggle");
                        }
                        if let Err(e) = self.refresh_menu() {
                            error!("Failed to refresh menu after toggle: {}", e);
                        }
                    }
                    Err(e) => error!("Failed to process toggle action '{}': {}", action, e),
                }
            } else if action.starts_with("reset_") {
                match action
                    .strip_prefix("reset_")
                    .ok_or_else(|| {
                        InvalidActionFormatSnafu {
                            action_string: action.clone(),
                            expected_prefix: "reset_",
                        }
                        .build()
                    })
                    .and_then(|s| {
                        s.parse::<usize>().context(ParseActionIndexSnafu {
                            action_string: s.to_string(),
                        })
                    }) {
                    Ok(index) => {
                        if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                            if let Some(task) = tasks.get_mut(index) {
                                if let Err(e) = task.reset() {
                                    error!("Failed to reset task {}: {}", task.name, e);
                                } else {
                                    info!("🔄 任务 '{}' 已重置", task.name);
                                }
                            } else {
                                error!("Task not found at index {} for reset", index);
                            }
                        } else {
                            error!("Failed to lock tasks for reset");
                        }
                        if let Err(e) = self.refresh_menu() {
                            error!("Failed to refresh menu after reset: {}", e);
                        }
                    }
                    Err(e) => error!("Failed to process reset action '{}': {}", action, e),
                }
            } else if action.starts_with("edit_") {
                warn!("✏️ 编辑功能待实现");
            } else if action.starts_with("delete_") {
                match action
                    .strip_prefix("delete_")
                    .ok_or_else(|| {
                        InvalidActionFormatSnafu {
                            action_string: action.clone(),
                            expected_prefix: "delete_",
                        }
                        .build()
                    })
                    .and_then(|s| {
                        s.parse::<usize>().context(ParseActionIndexSnafu {
                            action_string: s.to_string(),
                        })
                    }) {
                    Ok(index) => {
                        if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                            if index < tasks.len() {
                                let task_name = tasks.remove(index).name;
                                warn!("🗑️ 任务 '{}' 已删除", task_name);
                            } else {
                                error!("Task index {} out of bounds for delete", index);
                            }
                        } else {
                            error!("Failed to lock tasks for delete");
                        }
                        if let Err(e) = self.refresh_menu() {
                            error!("Failed to refresh menu after delete: {}", e);
                        }
                    }
                    Err(e) => error!("Failed to process delete action '{}': {}", action, e),
                }
            } else if action.starts_with("pin_") {
                match action
                    .strip_prefix("pin_")
                    .ok_or_else(|| {
                        InvalidActionFormatSnafu {
                            action_string: action.clone(),
                            expected_prefix: "pin_",
                        }
                        .build()
                    })
                    .and_then(|s| {
                        s.parse::<usize>().context(ParseActionIndexSnafu {
                            action_string: s.to_string(),
                        })
                    }) {
                    Ok(index) => {
                        let mut task_name_opt = None;
                        let mut is_pinned_opt = None;
                        if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                            if let Some(task) = tasks.get_mut(index) {
                                task.pinned = !task.pinned;
                                task_name_opt = Some(task.name.clone());
                                is_pinned_opt = Some(task.pinned);
                            } else {
                                error!("Task not found at index {} for pin/unpin", index);
                            }
                        } else {
                            error!("Failed to lock tasks for pin/unpin");
                        }

                        if let (Some(task_name), Some(is_pinned)) = (task_name_opt, is_pinned_opt) {
                            if is_pinned {
                                if let Err(e) = self.create_pinned_tray_icon(index) {
                                    error!("Failed to create pinned tray icon for task '{}': {}", task_name, e);
                                } else {
                                    info!("📌 任务 '{}' 已固定", task_name);
                                }
                            } else {
                                self.remove_pinned_tray_icon(index);
                                info!("📌 任务 '{}' 已取消固定", task_name);
                            }
                        }
                        if let Err(e) = self.refresh_menu() {
                            error!("Failed to refresh menu after pin/unpin: {}", e);
                        }
                    }
                    Err(e) => error!("Failed to process pin action '{}': {}", action, e),
                }
            } else if action.starts_with("unpin_") {
                match action
                    .strip_prefix("unpin_")
                    .ok_or_else(|| {
                        InvalidActionFormatSnafu {
                            action_string: action.clone(),
                            expected_prefix: "unpin_",
                        }
                        .build()
                    })
                    .and_then(|s| {
                        s.parse::<usize>().context(ParseActionIndexSnafu {
                            action_string: s.to_string(),
                        })
                    }) {
                    Ok(index) => {
                        let mut task_name_opt = None;
                        if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                            if let Some(task) = tasks.get_mut(index) {
                                task.pinned = false;
                                task_name_opt = Some(task.name.clone());
                            } else {
                                error!("Task not found at index {} for unpin", index);
                            }
                        } else {
                            error!("Failed to lock tasks for unpin");
                        }

                        if let Some(task_name) = task_name_opt {
                            self.remove_pinned_tray_icon(index);
                            info!("📌 任务 '{}' 已取消固定", task_name);
                        }
                        if let Err(e) = self.refresh_menu() {
                            error!("Failed to refresh menu after unpin: {}", e);
                        }
                    }
                    Err(e) => error!("Failed to process unpin action '{}': {}", action, e),
                }
            } else if action.starts_with("pinned_toggle_") {
                match action
                    .strip_prefix("pinned_toggle_")
                    .ok_or_else(|| {
                        InvalidActionFormatSnafu {
                            action_string: action.clone(),
                            expected_prefix: "pinned_toggle_",
                        }
                        .build()
                    })
                    .and_then(|s| {
                        s.parse::<usize>().context(ParseActionIndexSnafu {
                            action_string: s.to_string(),
                        })
                    }) {
                    Ok(index) => {
                        if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                            if let Some(task) = tasks.get_mut(index) {
                                if task.is_running {
                                    if let Err(e) = task.pause() {
                                        error!("Failed to pause pinned task {}: {}", task.name, e);
                                    } else {
                                        info!("⏸️ 固定任务 '{}' 已暂停", task.name);
                                    }
                                } else {
                                    task.start();
                                    info!("▶️ 固定任务 '{}' 已开始", task.name);
                                }
                            } else {
                                error!("Pinned task not found at index {} for toggle", index);
                            }
                        } else {
                            error!("Failed to lock tasks for pinned_toggle");
                        }
                        if let Err(e) = self.refresh_menu() {
                            error!("Failed to refresh menu after pinned_toggle: {}", e);
                        }
                        if let Err(e) = self.update_pinned_tray_icon(index) {
                            error!("Failed to update pinned tray icon after pinned_toggle: {}", e);
                        }
                    }
                    Err(e) => error!("Failed to process pinned_toggle action '{}': {}", action, e),
                }
            } else if action.starts_with("pinned_reset_") {
                match action
                    .strip_prefix("pinned_reset_")
                    .ok_or_else(|| {
                        InvalidActionFormatSnafu {
                            action_string: action.clone(),
                            expected_prefix: "pinned_reset_",
                        }
                        .build()
                    })
                    .and_then(|s| {
                        s.parse::<usize>().context(ParseActionIndexSnafu {
                            action_string: s.to_string(),
                        })
                    }) {
                    Ok(index) => {
                        if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                            if let Some(task) = tasks.get_mut(index) {
                                if let Err(e) = task.reset() {
                                    error!("Failed to reset pinned task {}: {}", task.name, e);
                                } else {
                                    info!("🔄 固定任务 '{}' 已重置", task.name);
                                }
                            } else {
                                error!("Pinned task not found at index {} for reset", index);
                            }
                        } else {
                            error!("Failed to lock tasks for pinned_reset");
                        }
                        if let Err(e) = self.refresh_menu() {
                            error!("Failed to refresh menu after pinned_reset: {}", e);
                        }
                        if let Err(e) = self.update_pinned_tray_icon(index) {
                            error!("Failed to update pinned tray icon after pinned_reset: {}", e);
                        }
                    }
                    Err(e) => error!("Failed to process pinned_reset action '{}': {}", action, e),
                }
            }
        } else {
            warn!("❌ 未找到菜单ID对应的动作: {:?}", menu_id);
            debug!("当前注册的所有菜单ID:");
            for (id, action) in &self.menu_ids {
                debug!("  {:?} -> {}", id, action);
            }
        }
    }

    /// 处理新建任务
    fn handle_new_task(&mut self) {
        info!("📝 开始新建任务");

        // 显示输入对话框
        let input = show_input_dialog(
            "新建任务",
            "请输入任务信息：\n\n格式示例：\n• 时间段：1h30m#学习\n• 截止时间：@19:00#工作\n\n其中 # \
             后面是任务名称（可选）",
            "1h#新任务",
        );

        match input {
            Some(user_input) => {
                info!("用户输入: {}", user_input);

                // 解析用户输入
                match parse_time_input(&user_input) {
                    Ok((task_name, task_type)) => {
                        // 创建新任务
                        match Task::new(task_name.clone(), task_type) {
                            Ok(new_task_obj) => {
                                // 添加到任务列表
                                if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                                    // Use TaskLockSnafu directly
                                    tasks.push(new_task_obj);
                                    info!("✅ 成功创建任务: {}", task_name);
                                } else {
                                    error!("❌ 无法获取任务列表锁 (new task)");
                                }
                            }
                            Err(e) => {
                                error!("❌ 创建任务对象失败 (Task::new failed): {}", e);
                            }
                        }
                        // 刷新菜单
                        if let Err(e) = self.refresh_menu() {
                            error!("Failed to refresh menu after new task attempt: {}", e);
                        } else {
                            info!("🔄 菜单已刷新 (new task attempt)");
                        }
                    }
                    Err(e) => {
                        // This is for parse_time_input error
                        error!("❌ 解析任务输入失败: {}", e);
                        // 显示错误信息给用户
                        #[cfg(target_os = "macos")]
                        {
                            let error_script = format!(
                                r#"display dialog "解析任务输入失败：\n\n{}\n\n请检查输入格式：\n• 时间段：1h30m#任务名\n• 截止时间：@19:00#任务名" with title "输入错误" buttons {{"确定"}} default button "确定" with icon stop"#,
                                e
                            );
                            match Command::new("osascript").arg("-e").arg(&error_script).output() {
                                Ok(_) => info!("Error dialog displayed for parse failure."),
                                Err(cmd_err) => error!("Failed to display error dialog via osascript: {}", cmd_err),
                            }
                        }
                    }
                }
            }
            None => {
                info!("用户取消了新建任务");
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        match event_loop.create_window(Window::default_attributes()) {
            Ok(_window) => {
                // Window created successfully
            }
            Err(e) => {
                error!("Failed to create window in resumed: {}", Error::WindowCreation {
                    source: e,
                    backtrace: Backtrace::capture()
                });
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn new_events(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop, cause: winit::event::StartCause) {
        if winit::event::StartCause::Init == cause {
            match self.new_tray_icon() {
                Ok(tray_icon) => self.tray_icon = Some(tray_icon),
                Err(e) => {
                    error!("Failed to create initial tray icon: {}", e);
                }
            }

            #[cfg(target_os = "macos")]
            unsafe {
                use objc2_core_foundation::CFRunLoop;
                match CFRunLoop::main().context(MacOsMainRunLoopUnavailableSnafu) {
                    // Use MacOsMainRunLoopUnavailableSnafu directly
                    Ok(rl) => CFRunLoop::wake_up(&rl),
                    Err(e) => error!("Failed to get main run loop in new_events: {}", e),
                }
            }
        }
    }

    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::TrayIconEvent(_) => {}
            UserEvent::MenuEvent(event) => {
                self.handle_menu_event(event);
            }
            UserEvent::UpdateTimer => {
                if let Err(e) = self.update_tray_icon() {
                    error!("Failed to update tray icon from timer: {}", e);
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1)));
            }
            UserEvent::StartTask(index) => {
                if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                    // Use TaskLockSnafu directly
                    if let Some(task) = tasks.get_mut(index) {
                        task.start();
                    } else {
                        error!("Task not found at index {} for StartTask", index);
                    }
                } else {
                    error!("Failed to lock tasks for StartTask");
                }
            }
            UserEvent::PauseTask(index) => {
                if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                    // Use TaskLockSnafu directly
                    if let Some(task) = tasks.get_mut(index) {
                        if let Err(e) = task.pause() {
                            error!("Failed to pause task {}: {}", task.name, e);
                        }
                    } else {
                        error!("Task not found at index {} for PauseTask", index);
                    }
                } else {
                    error!("Failed to lock tasks for PauseTask");
                }
            }
            UserEvent::ResetTask(index) => {
                if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                    // Use TaskLockSnafu directly
                    if let Some(task) = tasks.get_mut(index) {
                        if let Err(e) = task.reset() {
                            error!("Failed to reset task {}: {}", task.name, e);
                        }
                    } else {
                        error!("Task not found at index {} for ResetTask", index);
                    }
                } else {
                    error!("Failed to lock tasks for ResetTask");
                }
            }
            UserEvent::DeleteTask(index) => {
                if let Ok(mut tasks) = self.tasks.lock().map_err(|_| TaskLockSnafu.build()) {
                    // Use TaskLockSnafu directly
                    if index < tasks.len() {
                        tasks.remove(index);
                    } else {
                        error!("Task index {} out of bounds for DeleteTask", index);
                    }
                } else {
                    error!("Failed to lock tasks for DeleteTask");
                }
            }
        }
    }
}

fn format_remaining_time(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(target_os = "macos")]
fn show_input_dialog(title: &str, message: &str, default_text: &str) -> Option<String> {
    let script = format!(
        r#"display dialog "{}" with title "{}" default answer "{}" buttons {{"取消", "确定"}} default button "确定""#,
        message, title, default_text
    );

    let output_res = Command::new("osascript").arg("-e").arg(&script).output();

    match output_res {
        Ok(output) => {
            if output.status.success() {
                let output_str = String::from_utf8_lossy(&output.stdout);
                if let Some(text_part) = output_str.split("text returned:").nth(1) {
                    let user_input = text_part.trim().to_string();
                    if !user_input.is_empty() {
                        return Some(user_input);
                    }
                }
            }
            None
        }
        Err(e) => {
            error!("显示输入对话框失败 (osascript execution): {}", e);
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn show_input_dialog(title: &str, message: &str, default_text: &str) -> Option<String> {
    warn!("输入对话框在此平台不支持，使用默认值: '{}'", default_text);
    Some(default_text.to_string())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "time_ticker=debug,info".into()),
        )
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    info!("🚀 TimeTicker 应用程序启动");

    #[cfg(target_os = "macos")]
    {
        info!("🔧 预设置 Dock 图标，减少启动延迟");
        if let Err(e) = set_dock_visibility(true) {
            error!("Failed to set initial dock visibility: {}", e);
        }
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context(EventLoopCreationSnafu)?; // Use EventLoopCreationSnafu directly

    let proxy_tray_event = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let Err(e) = proxy_tray_event
            .send_event(UserEvent::TrayIconEvent(event))
            .context(EventLoopSendSnafu)
        {
            // Use EventLoopSendSnafu directly
            error!("Failed to send TrayIconEvent to event loop: {}", e);
        }
    }));

    let proxy_menu_event = event_loop.create_proxy();
    TrayMenuEvent::set_event_handler(Some(move |event| {
        if let Err(e) = proxy_menu_event
            .send_event(UserEvent::MenuEvent(event))
            .context(EventLoopSendSnafu)
        {
            // Use EventLoopSendSnafu directly
            error!("Failed to send MenuEvent to event loop: {}", e);
        }
    }));

    let mut app = Application::new();

    let proxy_timer = event_loop.create_proxy();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if let Err(e) = proxy_timer
                .send_event(UserEvent::UpdateTimer)
                .context(EventLoopSendSnafu)
            {
                // Use EventLoopSendSnafu directly
                error!(
                    "Failed to send UpdateTimer event to event loop: {}. Timer thread exiting.",
                    e
                );
                break;
            }
        }
    });

    event_loop.run_app(&mut app).context(EventLoopCreationSnafu)?; // Use EventLoopCreationSnafu directly

    Ok(())
}

fn load_icon(path: &std::path::Path) -> Result<tray_icon::Icon> {
    let image = image::open(path)
        .map_err(|e| Error::Image {
            source: e,
            backtrace: Backtrace::capture(),
        })?
        .into_rgba8();
    let (width, height) = image.dimensions();
    let rgba = image.into_raw();
    tray_icon::Icon::from_rgba(rgba, width, height).context(IconConversionSnafu) // Use IconConversionSnafu directly
}

#[cfg(target_os = "macos")]
fn set_dock_visibility(visible: bool) -> Result<()> {
    unsafe {
        let mtm = MainThreadMarker::new().context(MainThreadMarkerSnafu)?; // Use MainThreadMarkerSnafu directly
        let app = NSApplication::sharedApplication(mtm);
        let policy = if visible {
            NSApplicationActivationPolicy::Regular
        } else {
            NSApplicationActivationPolicy::Accessory
        };
        app.setActivationPolicy(policy);
        if visible {
            set_dock_icon()?;
            info!("✅ Dock 图标已显示，使用 dock.png");
        } else {
            info!("✅ Dock 图标已隐藏");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn set_dock_icon() -> Result<()> {
    use objc2::rc::Retained;
    unsafe {
        let mtm = MainThreadMarker::new().context(MainThreadMarkerSnafu)?; // Use MainThreadMarkerSnafu directly
        let app = NSApplication::sharedApplication(mtm);
        let dock_icon_path = std::path::Path::new("./assets/dock.png");
        if dock_icon_path.exists() {
            let absolute_path = std::fs::canonicalize(dock_icon_path).context(CanonicalizePathSnafu {
                path: dock_icon_path.to_path_buf(),
            })?; // Use CanonicalizePathSnafu directly
            let absolute_path_str = absolute_path.to_string_lossy();
            let path_str = NSString::from_str(&absolute_path_str);
            if let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &path_str) {
                app.setApplicationIconImage(Some(&image));
                info!("🖼️ 成功设置 Dock 图标为 dock.png");
            } else {
                warn!("⚠️ 无法加载 dock.png 图像文件");
                set_default_dock_icon()?;
            }
        } else {
            warn!("⚠️ 找不到 dock.png 文件: {}", dock_icon_path.display());
            set_default_dock_icon()?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn set_default_dock_icon() -> Result<()> {
    unsafe {
        let mtm = MainThreadMarker::new().context(MainThreadMarkerSnafu)?; // Use MainThreadMarkerSnafu directly
        let app = NSApplication::sharedApplication(mtm);
        app.setApplicationIconImage(None);
        info!("🔄 使用默认 Dock 图标");
    }
    Ok(())
}
