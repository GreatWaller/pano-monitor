//! 高性能 Rust RTSP 播放器
//!
//! 一个使用 GStreamer 和 egui 构建的桌面 RTSP 视频流播放器。
//!
//! # 架构概述
//!
//! 应用程序由以下模块组成：
//! - `video_source`: 封装 GStreamer 逻辑，负责 RTSP 流的解码和帧提取
//! - `app`: 封装 eframe/egui GUI 逻辑，负责用户界面和视频渲染
//!
//! # 线程模型
//!
//! - GStreamer 在其内部线程中处理 RTSP 流和解码
//! - appsink 回调在 GStreamer 线程中触发，将帧数据写入共享的 Arc<Mutex<>>
//! - GUI 主线程从共享内存中读取最新帧并渲染
//!
//! # 使用方法
//!
//! ```bash
//! cargo run
//! ```
//!
//! 在 GUI 中输入 RTSP 地址，点击"连接"按钮即可开始播放。

mod app;
mod video_source;

use app::RtspPlayerApp;
use video_source::VideoSource;
use gstreamer as gst;

fn main() -> eframe::Result<()> {
    // 初始化日志系统
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // 初始化 GStreamer
    // 这必须在创建任何 GStreamer 对象之前调用
    if let Err(e) = VideoSource::init() {
        log::error!("GStreamer 初始化失败：{:?}", e);
        eprintln!("错误：无法初始化 GStreamer");
        eprintln!("请确保已安装 GStreamer 1.x 及相关插件");
        eprintln!();
        eprintln!("安装指南:");
        eprintln!("  macOS: brew install gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-libav");
        eprintln!("  Ubuntu: sudo apt-get install libgstreamer1.0-dev gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav");
        eprintln!("  Windows: 从 https://gstreamer.freedesktop.org/download/ 下载安装");
        std::process::exit(1);
    }

    log::info!("GStreamer 初始化成功");
    log::info!("GStreamer 版本：{}", gst::version_string());

    // 配置原生窗口选项
    let native_options = eframe::NativeOptions {
        // 窗口大小
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([640.0, 480.0])
            .with_title("RTSP Player - 高性能 Rust 播放器"),
        ..Default::default()
    };

    // 启动 eframe 应用程序
    log::info!("启动 eframe 应用程序");
    
    eframe::run_native(
        "RTSP Player",
        native_options,
        Box::new(|cc| Ok(Box::new(RtspPlayerApp::new(cc)))),
    )
}
