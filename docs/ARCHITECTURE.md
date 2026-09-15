# 项目结构

当前保留两个独立二进制项目，未设置 Cargo workspace。根目录 Cargo 命令针对 Win32 原型；Slint 命令显式传入 `--manifest-path slint-preview/Cargo.toml`。

```text
note/
├── Cargo.toml / Cargo.lock      原生版依赖
├── src/
│   ├── main.rs                  原生版入口
│   └── ui.rs                    Win32 窗口、绘制与业务交互
├── slint-preview/
│   ├── Cargo.toml / Cargo.lock  Slint 版依赖
│   ├── build.rs                编译 Slint UI
│   ├── src/
│   │   ├── main.rs              数据、剪贴板、窗口定位与 UI 回调
│   │   ├── hotkey.rs            全局快捷键注册与消息线程
│   │   ├── reminders.rs         时间预设、到期判断与调度间隔
│   │   ├── drop_target.rs       Windows OLE 拖放目标与数据解析
│   │   └── drop_import.rs       后台文件复制与图片导入
│   └── ui/
│       ├── app.slint            小猫窗口、输入气泡、待办卡片
│       └── cat.svg              矢量小猫
└── docs/                       原生版说明、路线与结构说明
```

## Slint 运行方式

使用 winit 窗口后端与软件渲染器。小猫、完整面板和侧边速览是三个声明为无边框透明置顶的窗口；面板按显示器工作区定位。交互使用短动画和单次定时器。

普通输入通过 Slint 回调修改编辑缓冲，停顿 600 毫秒后保存。全局快捷键线程阻塞等待 Windows 消息，将收下请求投递到 Slint 主事件循环；读取剪贴板、保存和更新 UI 都由主线程执行。

`Store` 保存记录、草稿、当前编辑目标、缩略图缓存和一次性撤销状态。撤销检查截止时间、记录内容及编辑状态，并在写盘成功后结束撤销。

## 数据文件

Slint 数据目录为 `%LOCALAPPDATA%\PocketPetSlintPreview`：

- `state.json`：记录和草稿；可选 `remind_at` 为 Unix 秒数，旧数据缺省为不提醒。
- `state.backup.json`：上一次写入前的状态。
- `state.tmp`：写入临时文件，再替换正式文件。
- `images/`：剪贴板/拖放图片。
- `files/`：普通拖放附件；记录中可选 files 数组保存显示名与相对路径。移除引用或撤销不删除附件，保留备份引用有效性。

`--snapshot` 改用 `slint-preview/preview-output/`，使用示例数据生成真实渲染截图和输入事件检查结果。该目录不进入 Git。

当前 Slint 主文件仍集中包含多种职责。后续可将持久化与记录操作进一步提取成模块；本次整理保留现有源码路径，避免影响已有构建和运行方式。

提醒只维护未来事项的单次定时器，回调更新徽标和列表后重新计算间隔；无未来事项则停止。到期状态由记录时间与完成状态推导，不另存易失的通知标记。

拖放关闭 winit 默认文件接收器，由小猫的 OLE IDropTarget 统一处理一批文字/位图/文件。悬停只检查格式，松手后读取载荷；文件导入在单个后台线程进行，结果投递回 Slint 主线程保存记录。退出时撤销拖放注册并平衡 OLE 初始化。

## 新增模块与窗口边界

- rail.rs：侧边记录顺序、定位、悬停计时器、Windows 侧栏区域裁剪和回归检查。
- added_time.rs：加入时间显示。
- attention.rs：渐进退避的安静提醒策略。
- clipboard.rs：Windows 截图位图兼容处理。
- snap.rs：像素分层消散。

小猫原生裁剪、强制分层和边框校正已撤回，窗口属性交给 Slint。侧边窗口仍使用区域裁剪排除卡片空隙，重新展开时通过锁定版本的 i-slint-core 内部接口标记整窗重绘。升级 Slint 时需一并检查该接口和真实桌面显示；最新标题栏状态见 KNOWN_ISSUES.md；不要以内部快照替代桌面验证。

## 托盘管理

tray.rs 使用独立的阻塞 Win32 消息循环维护隐藏所有者窗口、通知区域图标和菜单；动作投递到 Slint 主线程后才访问 UI/Store。winit 的 skip_taskbar 和 owner_window 属性控制窗口收纳，不手工重写小猫的透明/边框样式。菜单重入时使用 Cell 存储可变状态，避免持有跨原生消息循环的独占引用。

正常启动先取得命名互斥对象；重复启动通过管理窗口投递唤回消息。测试预览跳过互斥，使用独立数据。主事件循环采用 run_event_loop_until_quit，全部窗口隐藏后仍可通过托盘恢复。退出时销毁图标和管理窗口，释放互斥句柄。

置顶设置写入独立 preferences.json，写入成功后更新三个 Slint topmost 属性。开机启动只操作当前用户的专用 Run 值，不需要管理员权限。启动时不自动注册。
