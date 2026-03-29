//! RTSP 视频源模块
//! 
//! 封装 GStreamer 逻辑，负责：
//! - 初始化 GStreamer
//! - 构建 RTSP 播放管道
//! - 通过 appsink 提取视频帧
//! - 将帧数据发送给 GUI 线程

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// 视频帧数据，包含 RGB 像素和尺寸信息
#[derive(Clone, Debug)]
pub struct VideoFrame {
    /// RGB 像素数据，按行优先存储
    pub data: Vec<u8>,
    /// 帧宽度（像素）
    pub width: u32,
    /// 帧高度（像素）
    pub height: u32,
}

/// 视频源状态
#[derive(Clone, Debug, PartialEq)]
pub enum VideoSourceState {
    /// 未连接
    Disconnected,
    /// 正在连接
    Connecting,
    /// 已连接并播放
    Playing,
    /// 错误状态
    Error(String),
}

/// 视频源配置
#[derive(Clone, Debug)]
pub struct VideoSourceConfig {
    /// RTSP URL 地址
    pub rtsp_url: String,
    /// 是否尝试使用硬件解码
    pub use_hw_decoder: bool,
}

/// 视频源管理器
/// 
/// 负责管理 GStreamer 管道生命周期和帧数据提取
pub struct VideoSource {
    /// GStreamer 管道元素
    pipeline: Option<gst::Pipeline>,
    /// 共享的视频帧数据，GUI 线程从中读取
    /// 使用 Arc<Mutex<>> 实现零拷贝的帧共享
    latest_frame: Arc<Mutex<Option<VideoFrame>>>,
    /// 运行状态标志
    is_running: Arc<AtomicBool>,
    /// 当前状态
    state: Arc<Mutex<VideoSourceState>>,
}

impl VideoSource {
    /// 创建新的视频源实例
    pub fn new() -> Self {
        Self {
            pipeline: None,
            latest_frame: Arc::new(Mutex::new(None)),
            is_running: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(VideoSourceState::Disconnected)),
        }
    }

    /// 初始化 GStreamer 库
    /// 
    /// 必须在创建任何 GStreamer 对象前调用
    pub fn init() -> Result<(), gst::glib::Error> {
        gst::init()
    }

    /// 获取当前视频帧的克隆
    /// 
    /// GUI 线程调用此方法获取最新帧进行渲染
    pub fn get_latest_frame(&self) -> Option<VideoFrame> {
        self.latest_frame.lock().ok().and_then(|opt| opt.clone())
    }

    /// 获取当前连接状态
    pub fn get_state(&self) -> VideoSourceState {
        self.state.lock().unwrap().clone()
    }

    /// 设置状态（内部使用）
    fn set_state(&self, state: VideoSourceState) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = state;
        }
    }

    /// 连接到 RTSP 流
    /// 
    /// # 参数
    /// * `config` - 视频源配置，包含 RTSP URL 和解码器偏好
    /// 
    /// # 返回
    /// 成功时返回 Ok，失败时返回错误信息
    pub fn connect(&mut self, config: &VideoSourceConfig) -> Result<(), String> {
        // 设置状态为连接中
        self.set_state(VideoSourceState::Connecting);
        self.is_running.store(true, Ordering::SeqCst);

        // 构建 GStreamer 管道
        let pipeline = self.build_pipeline(config)?;
        
        // 设置管道为播放状态
        pipeline.set_state(gst::State::Playing);

        // 处理总线消息（错误、EOS 等）
        self.setup_bus_watch(&pipeline);

        self.pipeline = Some(pipeline);
        Ok(())
    }

    /// 断开 RTSP 连接
    pub fn disconnect(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);

        if let Some(pipeline) = self.pipeline.take() {
            // 先设置为 Null 状态，停止所有数据处理
            pipeline.set_state(gst::State::Null);
        }

        // 清空帧数据
        if let Ok(mut frame) = self.latest_frame.lock() {
            *frame = None;
        }

        self.set_state(VideoSourceState::Disconnected);
    }

    /// 检查是否正在运行
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// 构建 GStreamer 管道
    /// 
    /// 管道结构：
    /// rtspsrc -> rtph264depay -> avdec_h264 -> videoconvert -> video/x-raw,format=RGB -> appsink
    /// 
    /// # 参数
    /// * `config` - 视频源配置
    /// 
    /// # 返回
    /// 成功时返回配置好的 Pipeline，失败时返回错误
    fn build_pipeline(&self, config: &VideoSourceConfig) -> Result<gst::Pipeline, String> {
        // 创建管道容器
        let pipeline = gst::Pipeline::new();

        // === 创建源元素 ===
        // rtspsrc: RTSP 源元素，负责从网络接收 RTSP 流
        let src = gst::ElementFactory::make("rtspsrc")
            .property("location", &config.rtsp_url)
            .property("is-live", true)           // 标记为直播流，启用适当的缓冲策略
            .property("tcp-timeout", 5000u32)    // TCP 超时时间（毫秒）
            .property("latency", 0u32)           // 延迟设置为 0，追求最低延迟
            .build()
            .map_err(|e| format!("无法创建 rtspsrc 元素：{:?}", e))?;

        // === 创建 RTP H264 解包器 ===
        // rtph264depay: 将 RTP 包解包成 H264 裸流
        let depay = gst::ElementFactory::make("rtph264depay")
            .build()
            .map_err(|e| format!("无法创建 rtph264depay 元素：{:?}", e))?;

        // === 创建 H264 解码器 ===
        // 根据配置选择硬件或软件解码器
        let decoder = if config.use_hw_decoder {
            // 尝试使用硬件解码器（NVIDIA/Intel/Apple）
            gst::ElementFactory::make("nvh264dec")
                .build()
                .or_else(|_| gst::ElementFactory::make("vaapih264dec").build())
                .or_else(|_| gst::ElementFactory::make("vtdec_h264").build())
                .unwrap_or_else(|_| {
                    // 硬件解码器不可用时回退到软件解码
                    log::warn!("硬件解码器不可用，回退到软件解码");
                    gst::ElementFactory::make("avdec_h264")
                        .build()
                        .expect("avdec_h264 应该始终可用")
                })
        } else {
            // avdec_h264: FFmpeg H264 软件解码器
            gst::ElementFactory::make("avdec_h264")
                .build()
                .map_err(|e| format!("无法创建 avdec_h264 解码器：{:?}", e))?
        };

        // === 创建视频转换器 ===
        // videoconvert: 视频格式转换元素
        let videoconvert = gst::ElementFactory::make("videoconvert")
            .build()
            .map_err(|e| format!("无法创建 videoconvert 元素：{:?}", e))?;

        // === 创建视频格式过滤器 ===
        // 强制输出为 RGB 格式，供 egui 使用
        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("format", "RGB")
                    .build(),
            )
            .build()
            .map_err(|e| format!("无法创建 capsfilter 元素：{:?}", e))?;

        // === 创建 appsink ===
        // appsink: 应用程序接收端，允许从管道中提取数据到 Rust 代码
        let appsink = gst::ElementFactory::make("appsink")
            .property("drop", true)              // 丢弃旧帧，防止积压
            .property("max-buffers", 1u32)       // 只保留最新一帧
            .property("emit-signals", true)      // 启用信号发射
            .build()
            .map_err(|e| format!("无法创建 appsink 元素：{:?}", e))?;

        // 将 appsink 转换为 gst_app::AppSink 以便连接信号
        let appsink_obj = appsink
            .clone()
            .downcast::<gst_app::AppSink>()
            .map_err(|_| "无法将 appsink 转换为 AppSink 类型")?;

        // === 添加所有元素到管道 ===
        let elements = [&src, &depay, &decoder, &videoconvert, &capsfilter, &appsink];
        pipeline.add_many(elements).map_err(|e| format!("添加元素到管道失败：{:?}", e))?;

        // === 链接元素 ===
        // rtspsrc 是动态元素，需要特殊处理
        // 先链接静态部分：depay -> decoder -> videoconvert -> capsfilter -> appsink
        gst::Element::link_many(&[
            &depay,
            &decoder,
            &videoconvert,
            &capsfilter,
            &appsink,
        ])
        .map_err(|e| format!("链接静态元素失败：{:?}", e))?;

        // 克隆共享数据引用供回调使用
        let frame_clone = Arc::clone(&self.latest_frame);
        let running_clone = Arc::clone(&self.is_running);

        // 连接 appsink 的 new-sample 信号
        // 当新帧到达时，此回调会被触发（在 GStreamer 线程中）
        appsink_obj.connect("new-sample", false, move |values| {
            if !running_clone.load(Ordering::SeqCst) {
                return Some(gst::FlowReturn::Flushing.to_value());
            }

            let sink = values[0].get::<gst_app::AppSink>().unwrap();

            // 从 appsink 拉取样本
            let sample = sink.pull_sample();

            match sample {
                Ok(sample) => {
                    // 获取样本的 buffer 和 caps
                    let buffer = sample.buffer().ok_or_else(|| {
                        log::error!("无法获取 buffer");
                        gst::FlowReturn::Error
                    }).unwrap();
                    let caps = sample.caps().unwrap();

                    // 解析视频尺寸
                    let structure = caps.structure(0).unwrap();
                    let width: i32 = structure.get("width").unwrap();
                    let height: i32 = structure.get("height").unwrap();

                    // 将 buffer 映射到 CPU 可访问的内存
                    let map = buffer.map_readable();

                    if let Ok(map) = map {
                        // 复制 RGB 数据
                        let data: Vec<u8> = map.as_slice().to_vec();

                        // 创建视频帧
                        let frame = VideoFrame {
                            data,
                            width: width as u32,
                            height: height as u32,
                        };

                        // 更新共享帧（非阻塞操作）
                        if let Ok(mut latest) = frame_clone.lock() {
                            *latest = Some(frame);
                        }
                    }

                    Some(gst::FlowReturn::Ok.to_value())
                }
                Err(_) => Some(gst::FlowReturn::Error.to_value()),
            }
        });

        // 处理 rtspsrc 的动态 pad
        // rtspsrc 会在连接后动态创建 sink pad
        let depay_sink_pad = depay
            .static_pad("sink")
            .ok_or("rtph264depay 没有 sink pad")?;
        
        src.connect_pad_added(move |_src, src_pad| {
            // 检查 pad 是否已链接
            if depay_sink_pad.is_linked() {
                log::warn!("depay sink pad 已链接，跳过");
                return;
            }
            
            // 链接 rtspsrc 的输出 pad 到 depay 的输入 pad
            if let Err(e) = src_pad.link(&depay_sink_pad) {
                log::error!("链接 rtspsrc 到 depay 失败：{:?}", e);
            }
        });

        Ok(pipeline)
    }

    /// 设置 GStreamer 总线消息监听
    ///
    /// 监听错误、警告、EOS 等消息
    fn setup_bus_watch(&mut self, pipeline: &gst::Pipeline) {
        let bus = pipeline.bus().unwrap();
        let state_clone = Arc::clone(&self.state);
        let is_running_clone = Arc::clone(&self.is_running);

        let _watch = bus.add_watch(move |_bus, msg| {
            match msg.view() {
                gst::MessageView::Error(err) => {
                    log::error!(
                        "GStreamer 错误：来自元素 {:?} - {}",
                        err.src().map(|s| s.path_string()),
                        err.error()
                    );
                    if let Ok(mut state) = state_clone.lock() {
                        *state = VideoSourceState::Error(format!(
                            "错误：{}",
                            err.error()
                        ));
                    }
                }
                gst::MessageView::Warning(warn) => {
                    log::warn!(
                        "GStreamer 警告：来自元素 {:?} - {}",
                        warn.src().map(|s| s.path_string()),
                        warn.error()
                    );
                }
                gst::MessageView::Eos(..) => {
                    log::info!("GStreamer 流结束 (EOS)");
                    if is_running_clone.load(Ordering::SeqCst) {
                        if let Ok(mut state) = state_clone.lock() {
                            *state = VideoSourceState::Disconnected;
                        }
                    }
                }
                gst::MessageView::StateChanged(state_changed) => {
                    log::debug!(
                        "状态变化：{:?} -> {:?}",
                        state_changed.old(),
                        state_changed.current()
                    );
                }
                _ => {}
            }
            true.into()
        });
    }
}

impl Default for VideoSource {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for VideoSource {
    fn drop(&mut self) {
        self.disconnect();
    }
}

// 实现 Send + Sync 以支持跨线程共享
unsafe impl Send for VideoSource {}
unsafe impl Sync for VideoSource {}
