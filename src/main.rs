#![allow(unused)]

mod task;
mod parser;

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, Instant};
use std::collections::HashMap;
use tray_icon::{
    menu::{Menu, MenuItem, PredefinedMenuItem, MenuId, MenuEvent as TrayMenuEvent, Submenu},
    TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconEventReceiver,
};
use winit::{
    application::ApplicationHandler,
    event::Event,
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
};
use task::{Task, TaskType};
use parser::parse_time_input;

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
    menu_ids: HashMap<MenuId, String>, // 菜单ID到动作的映射
    menu_items: HashMap<usize, Submenu>, // 任务索引到子菜单的映射，用于更新文本
    control_items: HashMap<usize, MenuItem>, // 任务索引到控制按钮的映射
}

impl Application {
    fn new() -> Application {
        // 创建一些测试任务
        let mut test_tasks = Vec::new();

        // 添加一个25分钟的番茄钟任务（暂停状态）
        test_tasks.push(Task::new(
            "番茄钟".to_string(),
            TaskType::Duration(Duration::from_secs(25 * 60))
        ));

        // 添加一个10分钟的休息任务（暂停状态）
        test_tasks.push(Task::new(
            "休息".to_string(),
            TaskType::Duration(Duration::from_secs(10 * 60))
        ));

        Application {
            tray_icon: None,
            tasks: Arc::new(Mutex::new(test_tasks)),
            menu_ids: HashMap::new(),
            menu_items: HashMap::new(),
            control_items: HashMap::new(),
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
        self.menu_ids.clear(); // 清除旧的菜单ID映射
        self.menu_items.clear(); // 清除旧的菜单项映射
        self.control_items.clear(); // 清除旧的控制项映射

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
                        self.menu_ids.insert(start_pause_id, format!("toggle_{}", i));
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
                task_submenu.append(&PredefinedMenuItem::separator()).unwrap();

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
                    if task.pinned { "取消固定" } else { "固定" },
                    true,
                    None
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
                            control_item.set_text(if task.is_running { "暂停" } else { "开始" });
                        }
                        _ => {}
                    }
                }
            }

            tray_icon.set_tooltip(Some(&tooltip)).unwrap();
        }
    }

    fn refresh_menu(&mut self) {
        let new_menu = self.build_menu();
        if let Some(tray_icon) = &self.tray_icon {
            tray_icon.set_menu(Some(Box::new(new_menu)));
        }
    }

    fn handle_menu_event(&mut self, event: TrayMenuEvent) {
        let menu_id = event.id;

        if let Some(action) = self.menu_ids.get(&menu_id).cloned() {
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
                            } else {
                                task.start();
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
                        }
                    }
                    self.refresh_menu(); // 刷新菜单以更新状态
                }
            } else if action.starts_with("edit_") {
                // 处理编辑
                println!("编辑功能待实现");
            } else if action.starts_with("delete_") {
                // 处理删除
                if let Ok(index) = action.strip_prefix("delete_").unwrap().parse::<usize>() {
                    if let Ok(mut tasks) = self.tasks.lock() {
                        if index < tasks.len() {
                            tasks.remove(index);
                        }
                    }
                    self.refresh_menu(); // 刷新菜单以移除已删除的任务
                }
            } else if action.starts_with("pin_") {
                // 处理固定/取消固定
                if let Ok(index) = action.strip_prefix("pin_").unwrap().parse::<usize>() {
                    if let Ok(mut tasks) = self.tasks.lock() {
                        if let Some(task) = tasks.get_mut(index) {
                            task.pinned = !task.pinned;
                            println!("任务 '{}' {}", task.name, if task.pinned { "已固定" } else { "已取消固定" });
                        }
                    }
                    self.refresh_menu(); // 刷新菜单以更新固定状态
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn new_events(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        if winit::event::StartCause::Init == cause {
            self.tray_icon = Some(self.new_tray_icon());
            
            #[cfg(target_os = "macos")]
            unsafe {
                use objc2_core_foundation::{CFRunLoop};
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
        println!("Error: {:?}", err);
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