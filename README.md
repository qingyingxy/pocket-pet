# 口袋宠物 Pocket Pet

面向临时待办和碎片信息的 Windows 桌面小工具：点小猫随手记，或用快捷键直接收下剪贴板内容，之后再处理。

目前以 **Rust + Slint 版**为主要迭代方向，保留 Rust/Win32 原型作为功能与资源占用对照。两个版本是独立 Cargo 项目，使用独立数据目录。

## 快速开始：Slint 版

需要 Windows、Rust 工具链和对应链接器。从仓库根目录执行（首次构建需要联网下载依赖）：

```powershell
cargo build --release --manifest-path slint-preview/Cargo.toml
.\slint-preview\target\release\pocket-pet-slint.exe
```

已构建时可直接双击该 exe。Cargo 不在 PATH 时，可用 `& "$env:USERPROFILE\.cargo\bin\cargo.exe"` 替代命令中的 `cargo`。

- **点小猫**：直接输入；Ctrl+V 粘贴文字或截图。
- **Enter**：收下；Shift+Enter 换行。
- **Ctrl+Alt+V**：全局快捷收下剪贴板；6 秒内点击小猫下方提示可撤销。
- **点待办数字**：查看、编辑、完成记录。
- **Esc / × / 切换其他窗口**：收起并保留草稿。

两版都使用 Ctrl+Alt+V，体验时请只运行一个实例。

## 项目导航

| 位置 | 内容 |
| --- | --- |
| [slint-preview/](slint-preview/README.md) | 当前 Slint 版操作、数据和开发说明 |
| [src/](src/) | 原生 Rust/Win32 原型源码 |
| [docs/NATIVE.md](docs/NATIVE.md) | 原生版完整使用说明 |
| [docs/ROADMAP.md](docs/ROADMAP.md) | 功能进度、验证边界与后续路线 |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 目录结构、事件和数据流程 |

当前已完成输入气泡简化、快捷收下与撤销；轻提醒、拖放尚未实现。

## 验证

先退出正在运行的应用，确保 Ctrl+Alt+V 未被占用，然后执行：

```powershell
cargo test --release --manifest-path slint-preview/Cargo.toml
.\slint-preview\target\release\pocket-pet-slint.exe --snapshot
```

测试覆盖快捷键消息接收与释放、保存/撤销回退、草稿与备份保护。`--snapshot` 会短暂打开窗口，在独立预览目录生成截图及输入检查结果后退出，不使用正式数据目录。真实输入法、截图粘贴、跨屏和焦点行为仍需实机验证。

## 数据与版本管理

- Slint 版：`%LOCALAPPDATA%\PocketPetSlintPreview`。
- 原生版：`%LOCALAPPDATA%\PocketPet`。
- 两版数据不自动迁移。删除程序前可备份整个对应目录，包括图片文件夹。
- Git 跟踪源码、文档、矢量资源和两个 `Cargo.lock`；不跟踪构建目录、预览输出、安装器和本地环境文件。
