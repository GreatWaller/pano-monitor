//! GUI 应用模块
//! 
//! 使用 eframe/egui 构建用户界面，负责：
//! - 显示视频帧
//! - 提供 RTSP 地址输入和连接控制
//! - 显示状态信息

use eframe::egui;
use std::sync::{Arc, Mutex};
use crossbeam::queue::ArrayQueue;

use crate::video_source::{VideoFrame, VideoSource, VideoSourceConfig, VideoSourceState};

/// 主应用程序结构体
pub struct RtspPlayerApp {
    /// 视频源管理器
    video_source: Arc<Mutex<VideoSource>>,
    /// 无锁视频帧队列
    frame_queue: Arc<ArrayQueue<VideoFrame>>,
    /// 无锁状态队列
    state_queue: Arc<ArrayQueue<VideoSourceState>>,
    /// RTSP 地址输入框内容
    rtsp_url: String,
    /// 是否正在连接/已连接
    is_connected: bool,
    /// 当前状态文本
    status_text: String,
    /// 是否使用硬件解码
    use_hw_decoder: bool,
    /// 帧计数（用于调试）
    frame_count: u64,
    /// 最后一帧的尺寸
    last_frame_size: Option<(u32, u32)>,
    /// 缓存的视频纹理
    texture: Option<egui::TextureHandle>,
    /// 当前状态（从队列中读取的最新状态）
    current_state: VideoSourceState,
}

impl RtspPlayerApp {
    /// 创建新的应用程序实例
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // 配置 egui 样式
        Self::configure_egui(cc);

        // 创建视频源，队列容量为 3 帧
        let video_source = VideoSource::new(3);
        // 获取队列的 Arc 引用，供 UI 直接访问（无锁）
        let frame_queue = video_source.get_frame_queue();
        let state_queue = video_source.get_state_queue();

        Self {
            video_source: Arc::new(Mutex::new(video_source)),
            frame_queue,
            state_queue,
            rtsp_url: String::new(),
            is_connected: false,
            status_text: String::from("未连接"),
            use_hw_decoder: false,
            frame_count: 0,
            last_frame_size: None,
            texture: None,
            current_state: VideoSourceState::Disconnected,
        }
    }

    /// 配置 egui 的显示选项
    fn configure_egui(cc: &eframe::CreationContext<'_>) {
        // 设置深色主题
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        // 配置字体以支持中文显示
        let mut fonts = egui::FontDefinitions::default();

        use egui::FontFamily;

        // macOS: 使用 Hiragino Sans GB
        #[cfg(target_os = "macos")]
        fonts.font_data.insert(
            "hiragino".to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(
                include_bytes!("/System/Library/Fonts/Hiragino Sans GB.ttc")
            )),
        );

        #[cfg(target_os = "macos")]
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "hiragino".to_owned());

        // Windows: 使用微软雅黑 (Microsoft YaHei)
        #[cfg(target_os = "windows")]
        fonts.font_data.insert(
            "msyh".to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(
                include_bytes!("C:/Windows/Fonts/msyh.ttc")
            )),
        );

        #[cfg(target_os = "windows")]
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "msyh".to_owned());

        cc.egui_ctx.set_fonts(fonts);
    }

    /// 尝试连接到 RTSP 流
    fn try_connect(&mut self) {
        if self.rtsp_url.trim().is_empty() {
            self.status_text = String::from("请输入 RTSP 地址");
            return;
        }

        let url = self.rtsp_url.clone();
        let use_hw = self.use_hw_decoder;

        // 克隆 Arc 以便在闭包中使用
        let video_source = Arc::clone(&self.video_source);
        let status_text = Arc::new(Mutex::new(self.status_text.clone()));
        let status_text_clone = Arc::clone(&status_text);

        // 在后台线程中执行连接操作（避免阻塞 UI）
        std::thread::spawn(move || {
            let config = VideoSourceConfig {
                rtsp_url: url,
                use_hw_decoder: use_hw,
            };

            let mut vs = video_source.lock().unwrap();
            match vs.connect(&config) {
                Ok(_) => {
                    log::info!("成功连接到 RTSP 流");
                }
                Err(e) => {
                    log::error!("连接失败：{}", e);
                    if let Ok(mut status) = status_text_clone.lock() {
                        *status = format!("错误：{}", e);
                    }
                }
            }
        });

        self.is_connected = true;
        self.status_text = String::from("正在连接...");
    }

    /// 断开 RTSP 连接
    fn disconnect(&mut self) {
        let mut vs = self.video_source.lock().unwrap();
        vs.disconnect();

        self.is_connected = false;
        self.status_text = String::from("已断开");
        self.frame_count = 0;
        self.last_frame_size = None;
        self.texture = None;
        self.current_state = VideoSourceState::Disconnected;
        
        // 清空队列
        while self.frame_queue.pop().is_some() {}
        while self.state_queue.pop().is_some() {}
    }

    /// 从视频源获取最新帧并转换为 egui 图像
    /// 返回 (ColorImage, 尺寸) 以便更新 last_frame_size
    fn get_frame_as_color_image(&mut self) -> Option<(egui::ColorImage, (u32, u32))> {
        // 从队列中获取最新帧（无锁操作）
        // 注意：这里我们只取一帧，如果需要获取多帧可以循环 pop
        self.frame_queue.pop().map(|frame| {
            let size = (frame.width, frame.height);
            let color_image = Self::convert_rgbx_to_color_image(frame);
            (color_image, size)
        })
    }

    /// 将 RGBx 视频帧转换为 egui::ColorImage
    ///
    /// RGBx 格式：每像素 4 字节 [R,G,B,x, R,G,B,x, ...]，x 是未使用的填充字节
    /// egui 使用 [r, g, b, a] 格式的预乘 alpha 颜色空间
    fn convert_rgbx_to_color_image(frame: VideoFrame) -> egui::ColorImage {
        let width = frame.width as usize;
        let height = frame.height as usize;

        // 分配图像数据缓冲区
        let mut pixels = vec![egui::Color32::BLACK; width * height];

        // 将 RGBx 数据转换为 egui 格式
        // RGBx 数据按行优先存储：[R,G,B,x, R,G,B,x, ...]
        for (y, row) in frame.data.chunks_exact(width * 4).enumerate() {
            for (x, rgbx) in row.chunks_exact(4).enumerate() {
                let idx = y * width + x;
                // RGBx 中 x 是填充字节，忽略它
                pixels[idx] = egui::Color32::from_rgb(rgbx[0], rgbx[1], rgbx[2]);
            }
        }

        egui::ColorImage {
            size: [width, height],
            pixels,
        }
    }

    /// 更新状态文本
    fn update_status(&mut self) {
        // 从状态队列中读取最新状态（无锁操作）
        while let Some(state) = self.state_queue.pop() {
            self.current_state = state;
        }

        self.status_text = match self.current_state {
            VideoSourceState::Disconnected => String::from("未连接"),
            VideoSourceState::Connecting => String::from("正在连接..."),
            VideoSourceState::Playing => format!("播放中 | 帧数：{}", self.frame_count),
            VideoSourceState::Error(ref msg) => format!("错误：{}", msg),
        };

        // 更新内部连接状态
        if matches!(self.current_state, VideoSourceState::Playing) && !self.is_connected {
            self.is_connected = true;
        }
    }
}

impl eframe::App for RtspPlayerApp {
    /// 主更新/渲染循环
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 更新状态信息
        self.update_status();

        // 顶部面板：控制栏
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // RTSP 地址输入框
                ui.label("RTSP 地址:");
                let text_edit = egui::TextEdit::singleline(&mut self.rtsp_url)
                    .hint_text("rtsp://192.168.1.220/CEN-Profile_101")
                    .desired_width(300.0);
                ui.add(text_edit);

                // 硬件解码选项
                ui.checkbox(&mut self.use_hw_decoder, "硬件解码");

                // 连接/断开按钮
                if self.is_connected {
                    if ui.button("断开").clicked() {
                        self.disconnect();
                    }
                } else {
                    if ui.button("连接").clicked() {
                        self.try_connect();
                    }
                }

                // 状态标签
                ui.separator();
                ui.label(&self.status_text);
            });
            ui.add_space(4.0);
        });

        // 中央面板：视频显示区域
        egui::CentralPanel::default().show(ctx, |ui| {
            // 尝试获取最新帧
            if let Some((color_image, frame_size)) = self.get_frame_as_color_image() {
                // 更新最后一帧的尺寸
                self.last_frame_size = Some(frame_size);

                // 计算保持宽高比的显示尺寸
                let available_size = ui.available_size();

                let display_size = Self::calculate_display_size(
                    frame_size.0 as f32,
                    frame_size.1 as f32,
                    available_size,
                );

                // 使用 TextureHandle 缓存纹理，避免每帧创建新纹理
                if let Some(texture) = &mut self.texture {
                    // 纹理已存在，更新内容
                    texture.set(color_image, egui::TextureOptions::LINEAR);
                } else {
                    // 首次创建纹理
                    self.texture = Some(ui.ctx().load_texture(
                        "video_frame",
                        color_image,
                        egui::TextureOptions::LINEAR,
                    ));
                }

                // 显示图像
                if let Some(texture) = &self.texture {
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(display_size)
                            .maintain_aspect_ratio(true),
                    );
                }
            } else {
                // 无视频时显示占位符
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(50.0);
                        ui.label(egui::RichText::new("📺").size(48.0));
                        ui.add_space(10.0);
                        ui.label("等待视频流...");
                        ui.label(egui::RichText::new(&self.status_text).size(14.0).color(egui::Color32::GRAY));
                    });
                });
            }
        });

        // 底部状态栏
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!(
                    "分辨率：{:?}",
                    self.last_frame_size.unwrap_or((0, 0))
                )).size(12.0));
                
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("RTSP Player v0.1.0").size(12.0).color(egui::Color32::GRAY));
                });
            });
        });

        // 请求重绘（持续刷新视频）
        // 在实际应用中，可以根据帧率调整重绘频率
        ctx.request_repaint();
    }
}

impl RtspPlayerApp {
    /// 计算保持宽高比的显示尺寸
    fn calculate_display_size(
        frame_width: f32,
        frame_height: f32,
        available_size: egui::Vec2,
    ) -> egui::Vec2 {
        let frame_aspect = frame_width / frame_height;
        let available_aspect = available_size.x / available_size.y;

        if frame_aspect > available_aspect {
            // 帧更宽，按宽度适配
            egui::vec2(available_size.x, available_size.x / frame_aspect)
        } else {
            // 帧更高，按高度适配
            egui::vec2(available_size.y * frame_aspect, available_size.y)
        }
    }
}
