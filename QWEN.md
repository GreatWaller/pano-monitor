# RTSP Player - 高性能 Rust 播放器

## 项目概述

一个使用 **GStreamer** 和 **egui/eframe** 构建的桌面 RTSP 视频流播放器。

### 技术栈

| 组件 | 技术 |
|------|------|
| GUI 框架 | eframe/egui 0.31 |
| 视频处理 | GStreamer 0.23 (gst-rs) |
| 编程语言 | Rust 2021 Edition |

### 架构模块

- **`video_source`** - 封装 GStreamer 逻辑，负责 RTSP 流的解码和帧提取
- **`app`** - 封装 eframe/egui GUI 逻辑，负责用户界面和视频渲染
- **`main`** - 应用程序入口，初始化 GStreamer 和 eframe

### 线程模型

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  GStreamer 线程  │────▶│   共享内存      │◀────│    GUI 主线程    │
│  (解码/帧提取)   │     │ Arc<Mutex<>>    │     │   (渲染/交互)    │
└─────────────────┘     └─────────────────┘     └─────────────────┘
```

- GStreamer 在其内部线程中处理 RTSP 流和解码
- `appsink` 回调在 GStreamer 线程中触发，将帧数据写入共享的 `Arc<Mutex<>>`
- GUI 主线程从共享内存中读取最新帧并渲染

## 构建和运行

### 前置要求

**GStreamer 依赖**（必须安装）：

```bash
# macOS
brew install gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-libav

# Ubuntu/Debian
sudo apt-get install libgstreamer1.0-dev gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav

# Windows
# 从 https://gstreamer.freedesktop.org/download/ 下载安装
```

### 编译命令

```bash
# 开发模式运行
cargo run

# 发布模式构建
cargo build --release

# 运行测试（如有）
cargo test
```

### 使用方法

1. 运行程序后，在 GUI 界面输入 RTSP 地址
2. 可选择启用"硬件解码"（优先使用 NVIDIA/Intel/Apple 硬件解码器）
3. 点击"连接"按钮开始播放

## 开发规范

### 代码风格

- 遵循 Rust 官方代码风格
- 使用 `rustfmt` 格式化代码
- 模块内部有详细的中文文档注释

### 日志

使用 `env_logger` + `log` crate，可通过环境变量控制日志级别：

```bash
# 设置日志级别
RUST_LOG=debug cargo run
RUST_LOG=info cargo run
```

### 中文字体支持

- **macOS**: 使用 Hiragino Sans GB (`/System/Library/Fonts/Hiragino Sans GB.ttc`)
- **Windows**: 使用微软雅黑 (`C:/Windows/Fonts/msyh.ttc`)

字体配置在 `src/app.rs` 的 `configure_egui()` 函数中，使用条件编译针对不同平台加载对应字体。

## 项目结构

```
pano-monitor/
├── Cargo.toml          # 项目配置和依赖
├── Cargo.lock          # 依赖锁定文件
├── .gitignore          # Git 忽略配置
├── QWEN.md             # 项目文档
└── src/
    ├── main.rs         # 程序入口，初始化逻辑
    ├── app.rs          # GUI 应用逻辑（eframe/egui）
    └── video_source.rs # GStreamer 视频源管理
```

## GStreamer 管道结构

```
rtspsrc → rtph264depay → [nvh264dec|vaapih264dec|vtdec_h264|avdec_h264] → videoconvert → capsfilter(RGB) → appsink
```

- `rtspsrc`: RTSP 源，接收网络流
- `rtph264depay`: RTP H.264 解包器
- 解码器：根据配置选择硬件或软件解码
- `videoconvert`: 视频格式转换
- `capsfilter`: 强制输出 RGB 格式供 egui 使用
- `appsink`: 应用端接收帧数据
