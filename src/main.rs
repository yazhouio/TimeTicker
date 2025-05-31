#![allow(unused)]

mod parser;
mod task;

use image::{ImageBuffer, Rgba, RgbaImage};
use parser::parse_time_input;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
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
    tray_icon:            Option<TrayIcon>,
    tasks:                Arc<Mutex<Vec<Task>>>,
    menu_ids:             HashMap<MenuId, String>, // 菜单ID到动作的映射
    menu_items:           HashMap<usize, Submenu>, // 任务索引到子菜单的映射，用于更新文本
    control_items:        HashMap<usize, MenuItem>, // 任务索引到控制按钮的映射
    pinned_tray_icons:    HashMap<usize, TrayIcon>, // 固定任务的独立托盘图标
    pinned_menu_items:    HashMap<usize, MenuItem>, // 固定托盘菜单中的时间显示项
    pinned_control_items: HashMap<usize, MenuItem>, // 固定托盘菜单中的控制按钮
}

impl Application {
    fn new() -> Application {
        // 创建一些测试任务
        let mut test_tasks = Vec::new();

        // 添加一个25分钟的番茄钟任务（暂停状态）
        test_tasks.push(Task::new(
            "番茄钟".to_string(),
            TaskType::Duration(Duration::from_secs(25 * 60)),
        ));

        // 添加一个10分钟的休息任务（暂停状态）
        test_tasks.push(Task::new(
            "休息".to_string(),
            TaskType::Duration(Duration::from_secs(10 * 60)),
        ));

        Application {
            tray_icon:            None,
            tasks:                Arc::new(Mutex::new(test_tasks)),
            menu_ids:             HashMap::new(),
            menu_items:           HashMap::new(),
            control_items:        HashMap::new(),
            pinned_tray_icons:    HashMap::new(),
            pinned_menu_items:    HashMap::new(),
            pinned_control_items: HashMap::new(),
        }
    }

    fn new_tray_icon(&mut self) -> TrayIcon {
        let path = "./assets/icon.jpg";
        let icon = load_icon(std::path::Path::new(path));

        let menu = self.build_menu();

        TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Time Ticker")
            .with_icon(icon)
            .with_title("⏰")
            .build()
            .unwrap()
    }

    fn build_menu(&mut self) -> Menu {
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
            let tasks = self.tasks.lock().unwrap();
            for (i, task) in tasks.iter().enumerate() {
                // 显示剩余时间的子菜单
                let time_str = format_remaining_time(task.get_remaining_time());
                let task_submenu = Submenu::new(&format!("{}#{}", time_str, task.name), true);
                self.menu_items.insert(i, task_submenu.clone()); // 存储子菜单引用

                // 根据任务类型添加不同的控制选项
                match task.task_type {
                    TaskType::Duration(_) => {
                        // 开始/暂停
                        let start_pause = MenuItem::new(
                            if task.is_running { "暂停" } else { "开始" },
                            true,
                            None,
                        );
                        let start_pause_id = start_pause.id().clone();
                        self.menu_ids
                            .insert(start_pause_id, format!("toggle_{}", i));
                        self.control_items.insert(i, start_pause.clone()); // 存储控制项引用
                        task_submenu.append(&start_pause).unwrap();

                        // 重置
                        let reset = MenuItem::new("重置", true, None);
                        let reset_id = reset.id().clone();
                        self.menu_ids.insert(reset_id, format!("reset_{}", i));
                        task_submenu.append(&reset).unwrap();
                    }
                    TaskType::Deadline(_) => {
                        // 截止时间类型任务不需要开始/暂停/重置
                    }
                }

                // 添加分隔线
                task_submenu
                    .append(&PredefinedMenuItem::separator())
                    .unwrap();

                // 新增任务
                let new_task = MenuItem::new("新增", true, None);
                let new_task_id = new_task.id().clone();
                self.menu_ids.insert(new_task_id, "new_task".to_string());
                task_submenu.append(&new_task).unwrap();

                // 编辑
                let edit = MenuItem::new("编辑", true, None);
                let edit_id = edit.id().clone();
                self.menu_ids.insert(edit_id, format!("edit_{}", i));
                task_submenu.append(&edit).unwrap();

                // 删除
                let delete = MenuItem::new("删除", true, None);
                let delete_id = delete.id().clone();
                self.menu_ids.insert(delete_id, format!("delete_{}", i));
                task_submenu.append(&delete).unwrap();

                // 固定/取消固定
                let pin = MenuItem::new(
                    if task.pinned {
                        "取消固定"
                    } else {
                        "固定"
                    },
                    true,
                    None,
                );
                let pin_id = pin.id().clone();
                self.menu_ids.insert(pin_id, format!("pin_{}", i));
                task_submenu.append(&pin).unwrap();

                // 将子菜单添加到主菜单
                menu.append(&task_submenu).unwrap();
            }
        }

        // 添加分隔线
        menu.append(&PredefinedMenuItem::separator()).unwrap();

        // 添加新建任务选项
        let new_task = MenuItem::new("新建任务", true, None);
        let new_task_id = new_task.id().clone();
        self.menu_ids.insert(new_task_id, "new_task".to_string());
        menu.append(&new_task).unwrap();

        // 添加退出选项
        let quit = MenuItem::new("退出", true, None);
        let quit_id = quit.id().clone();
        self.menu_ids.insert(quit_id, "quit".to_string());
        menu.append(&quit).unwrap();

        menu
    }

    fn update_tray_icon(&mut self) {
        if let Some(tray_icon) = &self.tray_icon {
            let tasks = self.tasks.lock().unwrap();
            let mut tooltip = String::new();

            // 更新tooltip和菜单项文本
            for (i, task) in tasks.iter().enumerate() {
                let remaining = task.get_remaining_time();
                let time_str = format_remaining_time(remaining);
                tooltip.push_str(&format!("{}#{}\n", time_str, task.name));

                // 更新菜单项文本（不会关闭菜单）
                if let Some(menu_item) = self.menu_items.get(&i) {
                    menu_item.set_text(&format!("{}#{}", time_str, task.name));
                }

                // 更新控制按钮文本
                if let Some(control_item) = self.control_items.get(&i) {
                    match task.task_type {
                        TaskType::Duration(_) => {
                            control_item.set_text(if task.is_running {
                                "暂停"
                            } else {
                                "开始"
                            });
                        }
                        _ => {}
                    }
                }
            }

            tray_icon.set_tooltip(Some(&tooltip)).unwrap();
        }

        // 更新所有固定的托盘图标
        let pinned_indices: Vec<usize> = self.pinned_tray_icons.keys().cloned().collect();
        for index in pinned_indices {
            self.update_pinned_tray_icon(index);
        }
    }

    fn refresh_menu(&mut self) {
        let new_menu = self.build_menu();
        if let Some(tray_icon) = &self.tray_icon {
            tray_icon.set_menu(Some(Box::new(new_menu)));
        }
    }

    fn create_pinned_tray_icon(&mut self, task_index: usize) {
        let path = "./assets/icon.jpg";
        let icon = load_icon(std::path::Path::new(path));

        // 先获取任务信息，然后释放锁
        let (task_name, task_type, is_running, remaining_time) = {
            let tasks = self.tasks.lock().unwrap();
            if let Some(task) = tasks.get(task_index) {
                (
                    task.name.clone(),
                    task.task_type.clone(),
                    task.is_running,
                    task.get_remaining_time(),
                )
            } else {
                return;
            }
        };

        // 现在可以安全地调用 build_pinned_task_menu
        let menu = self.build_pinned_task_menu(
            task_index,
            &task_name,
            &task_type,
            is_running,
            remaining_time,
        );

        // 使用时间文本作为标题，格式：MM:SS
        let time_str = format_remaining_time(remaining_time);
        let parts: Vec<&str> = time_str.split(':').collect();
        let time_title = if parts.len() >= 3 {
            format!("{}:{}", parts[1], parts[2]) // 显示 MM:SS
        } else {
            "00:00".to_string()
        };

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(&format!(
                "{}#{}",
                format_remaining_time(remaining_time),
                task_name
            ))
            .with_icon(icon)
            .with_title(&time_title)
            .build()
            .unwrap();

        self.pinned_tray_icons.insert(task_index, tray_icon);
    }

    fn build_pinned_task_menu(
        &mut self, task_index: usize, task_name: &str, task_type: &TaskType, is_running: bool,
        remaining_time: Duration,
    ) -> Menu {
        let menu = Menu::new();

        // 显示任务时间（正确显示当前剩余时间）
        let time_str = format_remaining_time(remaining_time);
        let time_item = MenuItem::new(&format!("{}#{}", time_str, task_name), false, None);
        self.pinned_menu_items.insert(task_index, time_item.clone()); // 保存引用以便更新
        menu.append(&time_item).unwrap();

        // 添加分隔线
        menu.append(&PredefinedMenuItem::separator()).unwrap();

        // 根据任务类型添加控制选项
        match task_type {
            TaskType::Duration(_) => {
                // 开始/暂停
                let start_pause =
                    MenuItem::new(if is_running { "暂停" } else { "开始" }, true, None);
                let start_pause_id = start_pause.id().clone();
                self.menu_ids
                    .insert(start_pause_id, format!("pinned_toggle_{}", task_index));
                self.pinned_control_items
                    .insert(task_index, start_pause.clone()); // 保存引用以便更新
                menu.append(&start_pause).unwrap();

                // 重置
                let reset = MenuItem::new("重置", true, None);
                let reset_id = reset.id().clone();
                self.menu_ids
                    .insert(reset_id, format!("pinned_reset_{}", task_index));
                menu.append(&reset).unwrap();
            }
            TaskType::Deadline(_) => {
                // 截止时间类型任务不需要开始/暂停/重置
            }
        }

        // 添加分隔线
        menu.append(&PredefinedMenuItem::separator()).unwrap();

        // 取消固定
        let unpin = MenuItem::new("取消固定", true, None);
        let unpin_id = unpin.id().clone();
        self.menu_ids
            .insert(unpin_id, format!("unpin_{}", task_index));
        menu.append(&unpin).unwrap();

        menu
    }

    fn remove_pinned_tray_icon(&mut self, task_index: usize) {
        self.pinned_tray_icons.remove(&task_index);
        self.pinned_menu_items.remove(&task_index);
        self.pinned_control_items.remove(&task_index);
    }

    fn update_pinned_tray_icon(&mut self, task_index: usize) {
        // 先获取任务信息
        let (task_name, task_type, is_running, remaining_time) = {
            if let Ok(tasks) = self.tasks.lock() {
                if let Some(task) = tasks.get(task_index) {
                    (
                        task.name.clone(),
                        task.task_type.clone(),
                        task.is_running,
                        task.get_remaining_time(),
                    )
                } else {
                    return;
                }
            } else {
                return;
            }
        };

        // 更新托盘图标
        if let Some(tray_icon) = self.pinned_tray_icons.get(&task_index) {
            let time_str = format_remaining_time(remaining_time);
            let tooltip = format!("{}#{}", time_str, task_name);

            // 使用文本标题显示时间，格式：MM:SS
            let parts: Vec<&str> = time_str.split(':').collect();
            let time_title = if parts.len() >= 3 {
                format!("{}:{}", parts[1], parts[2]) // 显示 MM:SS
            } else {
                "00:00".to_string()
            };

            tray_icon.set_title(Some(&time_title));
            tray_icon.set_tooltip(Some(&tooltip));
        }

        // 更新固定菜单中的时间显示项（不重新构建菜单，避免菜单消失）
        if let Some(menu_item) = self.pinned_menu_items.get(&task_index) {
            let time_str = format_remaining_time(remaining_time);
            menu_item.set_text(&format!("{}#{}", time_str, task_name));
        }

        // 更新固定菜单中的控制按钮文本
        if let Some(control_item) = self.pinned_control_items.get(&task_index) {
            if let TaskType::Duration(_) = task_type {
                control_item.set_text(if is_running { "暂停" } else { "开始" });
            }
        }
    }

    fn create_time_icon(&self, time_str: &str) -> Icon {
        // 直接使用简化版本，绘制数字时间
        self.create_digital_time_icon(time_str)
    }

    fn create_digital_time_icon(&self, time_str: &str) -> Icon {
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
            let display_time = format!("{}:{}", minutes, seconds);
            self.draw_large_text(&mut img, &display_time, 1, 10);
        } else {
            // 如果解析失败，显示时钟图标
            self.draw_clock_icon(&mut img);
        }

        // 转换为Icon
        let rgba_data = img.into_raw();
        Icon::from_rgba(rgba_data, width, height).unwrap()
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

                if distance >= 6.0 && distance <= 8.0 {
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

    fn draw_pattern(
        &self, img: &mut RgbaImage, x: u32, y: u32, pattern: &[[u8; 3]; 5], color: Rgba<u8>,
    ) {
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
    fn draw_large_pattern(
        &self, img: &mut RgbaImage, x: u32, y: u32, pattern: &[[u8; 5]; 7], color: Rgba<u8>,
    ) {
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

    fn handle_menu_event(&mut self, event: TrayMenuEvent) {
        let menu_id = event.id;

        debug!("菜单事件触发，ID: {:?}", menu_id);

        if let Some(action) = self.menu_ids.get(&menu_id).cloned() {
            debug!("找到对应动作: {}", action);
            if action == "quit" {
                std::process::exit(0);
            } else if action == "new_task" {
                // TODO: 实现新建任务
                println!("新建任务功能待实现");
            } else if action.starts_with("task_") {
                // 处理任务点击
                println!("点击了任务");
            } else if action.starts_with("toggle_") {
                // 处理开始/暂停
                if let Ok(index) = action.strip_prefix("toggle_").unwrap().parse::<usize>() {
                    if let Ok(mut tasks) = self.tasks.lock() {
                        if let Some(task) = tasks.get_mut(index) {
                            if task.is_running {
                                task.pause();
                                info!("⏸️ 任务 '{}' 已暂停", task.name);
                            } else {
                                task.start();
                                info!("▶️ 任务 '{}' 已开始", task.name);
                            }
                        }
                    }
                    self.refresh_menu(); // 刷新菜单以更新按钮文本
                }
            } else if action.starts_with("reset_") {
                // 处理重置
                if let Ok(index) = action.strip_prefix("reset_").unwrap().parse::<usize>() {
                    if let Ok(mut tasks) = self.tasks.lock() {
                        if let Some(task) = tasks.get_mut(index) {
                            task.reset();
                            info!("🔄 任务 '{}' 已重置", task.name);
                        }
                    }
                    self.refresh_menu(); // 刷新菜单以更新状态
                }
            } else if action.starts_with("edit_") {
                // 处理编辑
                warn!("✏️ 编辑功能待实现");
            } else if action.starts_with("delete_") {
                // 处理删除
                if let Ok(index) = action.strip_prefix("delete_").unwrap().parse::<usize>() {
                    if let Ok(mut tasks) = self.tasks.lock() {
                        if index < tasks.len() {
                            let task_name = tasks[index].name.clone();
                            tasks.remove(index);
                            warn!("🗑️ 任务 '{}' 已删除", task_name);
                        }
                    }
                    self.refresh_menu(); // 刷新菜单以移除已删除的任务
                }
            } else if action.starts_with("pin_") {
                // 处理固定/取消固定
                if let Ok(index) = action.strip_prefix("pin_").unwrap().parse::<usize>() {
                    let (task_name, is_pinned) = {
                        if let Ok(mut tasks) = self.tasks.lock() {
                            if let Some(task) = tasks.get_mut(index) {
                                task.pinned = !task.pinned;
                                (task.name.clone(), task.pinned)
                            } else {
                                return;
                            }
                        } else {
                            return;
                        }
                    };

                    if is_pinned {
                        // 创建独立的托盘图标
                        self.create_pinned_tray_icon(index);
                        info!("📌 任务 '{}' 已固定，创建了独立托盘图标", task_name);
                    } else {
                        // 移除独立的托盘图标
                        self.remove_pinned_tray_icon(index);
                        info!("📌 任务 '{}' 已取消固定，移除了独立托盘图标", task_name);
                    }

                    self.refresh_menu(); // 刷新菜单以更新固定状态
                }
            } else if action.starts_with("unpin_") {
                // 处理从固定托盘图标取消固定
                if let Ok(index) = action.strip_prefix("unpin_").unwrap().parse::<usize>() {
                    let task_name = {
                        if let Ok(mut tasks) = self.tasks.lock() {
                            if let Some(task) = tasks.get_mut(index) {
                                task.pinned = false;
                                task.name.clone()
                            } else {
                                return;
                            }
                        } else {
                            return;
                        }
                    };

                    // 移除独立的托盘图标
                    self.remove_pinned_tray_icon(index);
                    info!("📌 任务 '{}' 已取消固定，移除了独立托盘图标", task_name);

                    self.refresh_menu(); // 刷新主菜单以更新固定状态
                }
            } else if action.starts_with("pinned_toggle_") {
                // 处理固定托盘图标的开始/暂停
                if let Ok(index) = action
                    .strip_prefix("pinned_toggle_")
                    .unwrap()
                    .parse::<usize>()
                {
                    if let Ok(mut tasks) = self.tasks.lock() {
                        if let Some(task) = tasks.get_mut(index) {
                            if task.is_running {
                                task.pause();
                                info!("⏸️ 固定任务 '{}' 已暂停", task.name);
                            } else {
                                task.start();
                                info!("▶️ 固定任务 '{}' 已开始", task.name);
                            }
                        }
                    }
                    // 刷新主菜单和固定托盘图标
                    self.refresh_menu();
                    self.update_pinned_tray_icon(index);
                }
            } else if action.starts_with("pinned_reset_") {
                // 处理固定托盘图标的重置
                if let Ok(index) = action
                    .strip_prefix("pinned_reset_")
                    .unwrap()
                    .parse::<usize>()
                {
                    if let Ok(mut tasks) = self.tasks.lock() {
                        if let Some(task) = tasks.get_mut(index) {
                            task.reset();
                            info!("🔄 固定任务 '{}' 已重置", task.name);
                        }
                    }
                    // 刷新主菜单和固定托盘图标
                    self.refresh_menu();
                    self.update_pinned_tray_icon(index);
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
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let window = event_loop
            .create_window(Window::default_attributes())
            .unwrap();
    }

    fn window_event(
        &mut self, _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId, _event: winit::event::WindowEvent,
    ) {
    }

    fn new_events(
        &mut self, _event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        if winit::event::StartCause::Init == cause {
            self.tray_icon = Some(self.new_tray_icon());

            #[cfg(target_os = "macos")]
            unsafe {
                use objc2_core_foundation::CFRunLoop;
                let rl = CFRunLoop::main().unwrap();
                CFRunLoop::wake_up(&rl);
            }
        }
    }

    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::TrayIconEvent(_) => {
                // 处理托盘图标事件
            }
            UserEvent::MenuEvent(event) => {
                self.handle_menu_event(event);
            }
            UserEvent::UpdateTimer => {
                self.update_tray_icon(); // 现在使用set_text()更新，不会关闭菜单
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_secs(1),
                ));
            }
            UserEvent::StartTask(index) => {
                if let Ok(mut tasks) = self.tasks.lock() {
                    if let Some(task) = tasks.get_mut(index) {
                        task.start();
                    }
                }
            }
            UserEvent::PauseTask(index) => {
                if let Ok(mut tasks) = self.tasks.lock() {
                    if let Some(task) = tasks.get_mut(index) {
                        task.pause();
                    }
                }
            }
            UserEvent::ResetTask(index) => {
                if let Ok(mut tasks) = self.tasks.lock() {
                    if let Some(task) = tasks.get_mut(index) {
                        task.reset();
                    }
                }
            }
            UserEvent::DeleteTask(index) => {
                if let Ok(mut tasks) = self.tasks.lock() {
                    tasks.remove(index);
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
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

fn main() {
    // 初始化 tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "time_ticker=debug,info".into()),
        )
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    info!("🚀 TimeTicker 应用程序启动");

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().unwrap();

    // 设置托盘事件处理器
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        proxy.send_event(UserEvent::TrayIconEvent(event));
    }));

    let proxy = event_loop.create_proxy();
    TrayMenuEvent::set_event_handler(Some(move |event| {
        proxy.send_event(UserEvent::MenuEvent(event));
    }));

    let mut app = Application::new();

    // 设置定时器更新
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            proxy.send_event(UserEvent::UpdateTimer).unwrap();
        }
    });

    if let Err(err) = event_loop.run_app(&mut app) {
        error!("💥 应用程序运行错误: {:?}", err);
    }
}

fn load_icon(path: &std::path::Path) -> tray_icon::Icon {
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::open(path)
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    tray_icon::Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("Failed to open icon")
}
