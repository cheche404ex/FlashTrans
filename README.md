# FlashTrans 2

FlashTrans is released under the [MIT License](LICENSE).

FlashTrans 2 是 Windows 在线翻译与 OCR 工作台，使用 Tauri 2、Rust 与 Vanilla TypeScript/Vite 构建。

本项目仅使用用户自行配置的 OpenAI 兼容 API。

## 功能

- `F1`：开关会话级划词翻译。
- `F2`：输入文字并翻译，可把译文粘贴回原窗口。
- `F3`：框选屏幕区域，选择 Windows OCR 或模型视觉 OCR，再调用当前 API 预设翻译。
- `F4`：打开使用当前在线 API 预设的 AI 对话窗口。
- `F5`：显示主工作台。
- 多个 OpenAI 兼容 API 预设，可快速切换；视觉能力通过 `supportsVision` 手动声明。
- 多个领域提示词预设，适用于划词、打字、截图和主窗口翻译。
- 可拖动、无级缩放并记忆尺寸的翻译悬浮窗。

所有热键均可在设置中修改。`F1` 开关只在当前会话生效，应用重启后默认关闭。

## OCR

截图 OCR 有两种互斥模式：

- Windows OCR：使用系统 `Windows.Media.Ocr` 在本机识别截图，不上传图像。
- 模型视觉：把截图发送给当前 API 预设，由视觉模型提取文字，再发起独立翻译请求。

## 隐私与网络

- 翻译和对话请求会把用户输入发送到当前配置的 API Base URL。
- 模型视觉 OCR 会把所选截图发送到当前配置的 API Base URL。
- Windows OCR 不上传截图，但后续翻译会发送 OCR 识别出的文字。

实际隐私与数据保留策略还取决于用户选择的 API 服务商，请同时查阅该服务商条款。
