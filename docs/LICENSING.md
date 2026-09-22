# 许可与素材说明

## 本项目

Pocket Pet 自有源码、文档及自有 SVG 素材采用 [MIT License](../LICENSE)。第三方依赖保留各自许可证，MIT 不替代第三方许可条件。

cat.svg 与 check.svg 按维护者说明，属于开发过程中自行生成、AI 辅助制作的素材。该来源说明不表示完成了独立权利审计。本项目没有附带 Codex 宠物素材，也不是 OpenAI/Codex 官方产品。

像素消散设计参考 Snappable 思路，使用 Rust/Slint 实现，不依赖 Flutter/Snappable 软件包。

## Slint

Slint 1.17.1 提供 GPL-3.0-only、LicenseRef-Slint-Royalty-free-2.0 或 LicenseRef-Slint-Software-3.0 许可选项。

本项目是 Windows 桌面应用，使用 **Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0**。依赖附带的条款副本见 [Slint 许可原文](licenses/Slint-Royalty-free-2.0.md)。

该许可允许将 Slint 作为桌面应用的一部分使用和分发，并要求署名。本项目采用第 2(b) 条方案，在公开项目首页展示官方 Made with Slint 徽标。另建二进制下载页面时，应保留易于找到的徽标，或在应用中提供符合第 2(a) 条的 AboutSlint。

不能将这套方案直接套用于独立分发 Slint 工具包、暴露其 API 的产品或嵌入式系统。升级依赖时需重新核对适用条款。

- [Slint 官方许可说明](https://slint.dev/license/)
- [官方署名徽标来源](https://github.com/slint-ui/slint/blob/master/logo/MadeWithSlint-logo-whitebg.png)

## 其他依赖

两个 Cargo.lock 记录实际依赖版本。本阶段只发布源码，不发布预编译二进制；后续安装包应针对实际构建收集第三方许可证和 notices。本说明不是完整的二进制分发清单。

快捷方式的 cat.ico 由现有 cat.svg 渲染生成，沿用其许可，可通过 examples/make_icon.rs 重新生成。本地安装脚本复制项目及 Slint 许可，尚不是公开发布安装包的完整第三方 notices 清单。
