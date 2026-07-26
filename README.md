# FlashTrans 2

FlashTrans is released under the [MIT License](LICENSE).

FlashTrans 2 是 Windows 在线翻译与 OCR 工作台，使用 Tauri 2、Rust 与 Vanilla TypeScript/Vite 构建。

本项目仅使用用户自行配置的 OpenAI 兼容 API。仓库和发布包均不需要 Python、GGUF、NLLB、Opus-MT、CTranslate2、CMake 或 libclang。

## 功能

- `F1`：开关会话级划词翻译。开启后监听鼠标划选，优先通过 Windows UI Automation 读取选区，失败时使用复制法并恢复原剪贴板内容。
- `F2`：输入文字并翻译，可把译文粘贴回原窗口。
- `F3`：框选屏幕区域，选择 Windows OCR 或模型视觉 OCR，再调用当前 API 预设翻译。
- `F4`：打开使用当前在线 API 预设的 AI 对话窗口。
- `F5`：显示主工作台。
- 多个 OpenAI 兼容 API 预设，可快速切换；视觉能力通过 `supportsVision` 手动声明，不进行自动探测。
- 多个领域提示词预设，适用于划词、打字、截图和主窗口翻译。
- 可拖动、无级缩放并记忆尺寸的翻译悬浮窗，每次显示时按当前光标重新定位。
- 单实例运行；二次启动会显示、取消最小化并聚焦已有主窗口。

所有热键均可在设置中修改。`F1` 开关只在当前会话生效，应用重启后默认关闭。

## OCR

截图 OCR 有两种互斥模式：

- Windows OCR：使用系统 `Windows.Media.Ocr` 在本机识别截图，不上传图像。
- 模型视觉：把截图发送给当前 API 预设，由视觉模型提取文字，再发起独立翻译请求。当前预设必须手动开启“模型支持视觉”；识别失败时直接报错，不自动回退 Windows OCR。

## 隐私与网络

- 设置和 API Key 保存在 `%APPDATA%\com.chai1220.flashtrans\settings.json`，不会由应用上传到其他服务。
- 翻译和对话请求会把用户输入发送到当前配置的 API Base URL。
- 模型视觉 OCR 会把所选截图发送到当前配置的 API Base URL。
- Windows OCR 不上传截图，但后续翻译会发送 OCR 识别出的文字。
- 应用无遥测、无使用统计、无自动模型下载；除用户发起的 API 请求外，不主动联网。

实际隐私与数据保留策略还取决于用户选择的 API 服务商，请同时查阅该服务商条款。

## 开发环境

需要：

- Windows 10 或 Windows 11
- Node.js 18 或更高版本
- Rust stable，目标为 `x86_64-pc-windows-msvc`
- Visual Studio 2022 Build Tools 的“使用 C++ 的桌面开发”工作负载和 Windows 10/11 SDK
- Microsoft Edge WebView2 Runtime

安装依赖并启动开发版：

```powershell
npm install
npm run tauri dev
```

仅检查前端：

```powershell
npm run build
```

检查 Rust 与运行测试：

```powershell
Set-Location src-tauri
cargo check
cargo test
```

## 发布构建

```powershell
npm ci
npm run tauri build
```

可执行文件生成在 `src-tauri/target/release/FlashTrans.exe`，Tauri 安装包生成在 `src-tauri/target/release/bundle/`。

当前仓库没有 `installer/FlashTrans.iss`，因此不应再执行旧 README 中的 Inno Setup、Python bridge 或 PyInstaller 步骤。若后续需要自定义安装器，应新增并单独维护对应的发布脚本；默认发布链路以 Tauri bundler 产物为准。

本项目为改造和验收而新增的开发工具、磁盘占用及最终卸载步骤记录在 [../DEVELOPMENT_TOOLS_CLEANUP.md](../DEVELOPMENT_TOOLS_CLEANUP.md)。后续安装任何新工具时必须同步更新该文档。

## 发布前检查

1. 执行 `npm run build`、`cargo check` 和 `cargo test`。
2. 执行 `npm run tauri build`，确认 exe 与安装包可启动。
3. 验证二次启动只激活现有主窗口。
4. 在记事本、浏览器和至少一个 Electron 应用中验证 F1 划词、剪贴板恢复、去重和最新请求优先。
5. 分别验证 Windows OCR 与模型视觉 OCR；模型视觉失败时不应自动回退。
6. 在多显示器和不同缩放比例下验证 popup 拖动、缩放、尺寸记忆和重新定位。
7. 验证浅色/深色主题、F2/F3/F4/F5 与托盘菜单。
