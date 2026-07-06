# AGENTS.md — Hoshineko File Manager

正在大幅度重构: electron -> tauri

- 前端(vite, react, material web, lit): `src/`
- 后端(rust, tokio): `src-tauri/`

## 注意事项

每次更改都要写 devlog !!

### 前端

一定要写jsdoc！写字段说明！！！写i18n！！

先检查会不会破坏别的代码逻辑和功能。会的话跟我说！！

### 后端

添加依赖用 `cargo add` 而不是编辑 `Cargo.toml`。依赖版本应该用最新的。

最高的优先级是安全和性能！

写注释，写文档！

不要写太长的函数，不同的功能要拆分到不同的文件！\

先检查会不会破坏别的代码逻辑和功能。会的话跟我说！！
