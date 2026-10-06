# 开发与反馈

当前优先目标是快速记录、少打断和稳定的 Windows 桌面交互。提交新功能前，建议先说明它能减少哪一步操作，以及是否会增加常驻资源开销。

## 本地开发

从仓库根目录显式指定 `slint-preview/Cargo.toml`，根目录 Cargo 项目是早期 Win32 原型。

```powershell
cargo fmt --manifest-path slint-preview/Cargo.toml -- --check
cargo test --locked --manifest-path slint-preview/Cargo.toml
cargo build --release --locked --manifest-path slint-preview/Cargo.toml
```

测试全局快捷键前退出应用。UI 修改还应运行 `--snapshot` 并检查对应 `*-check.txt`；窗口相关修改必须额外实机检查启动、悬停、点击展开、拖动和退出。不要将内部快照当作最终桌面画面的证明。

可用 `./scripts/verify.ps1 -Offline` 串起上述检查和四组隔离预览，检查报告缺失及 FAIL；小猫边界组检查原生区域与窗口命中，`-NativeHover` 会短暂移动实际鼠标。可用 `./scripts/benchmark.ps1 -Offline` 测量模拟场景资源占用，`-Images` 增加图片。验证边界与复现方式见 [VALIDATION.md](docs/VALIDATION.md) 和 [PERFORMANCE.md](docs/PERFORMANCE.md)。

## 报告问题

请先查看已知问题，描述期望行为、实际行为、复现步骤，并提供 Windows 版本、缩放比例和显示器数量。截图、日志和示例文件请移除私人待办、联系人与路径。

## 修改范围

尽量保持改动集中，注明验证方式和未验证场景。不要提交 target、preview-output、真实 state.json、附件、个人路径、密钥或诊断日志。涉及窗口状态时避免同时由框架和原生补丁管理同一属性。

提交贡献时，请确认你有权提供这些内容，并同意自有贡献按项目 MIT 许可证发布。引入第三方代码或素材需记录来源及许可，不能直接改标为 MIT。
