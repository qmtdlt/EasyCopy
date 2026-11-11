# EasyCopy

Mac 与 Windows 双向剪贴板同步。Rust 核心与 Linux 中转服务，Tauri 2 + Vue 3 桌面客户端。文本用 UTF-8，图片内容用 PNG；文件与目录流式封装 TAR，接收端校验、还原到本地缓存后写入系统文件引用。

## 开源许可

本项目采用 [MIT 许可证](LICENSE)，允许使用、修改、分发及商业使用（包括闭源产品），需保留版权和许可声明。第三方依赖遵循各自的许可证。

使用者需自行部署中转服务。项目不提供公共中转服务器；部署记录中的内网地址及本机路径仅用于记录维护者的环境，请替换为自己的配置。不要提交配对串、私钥、客户端凭据或运行数据。

## 安装包与发布

最新正式版下载：[Mac Apple Silicon](https://github.com/qmtdlt/EasyCopy/releases/latest/download/EasyCopy_macos_arm64.dmg) · [Mac Intel](https://github.com/qmtdlt/EasyCopy/releases/latest/download/EasyCopy_macos_x64.dmg) · [Windows x64](https://github.com/qmtdlt/EasyCopy/releases/latest/download/EasyCopy_windows_x64_setup.exe)。[桌面更新说明与校验清单](https://github.com/qmtdlt/EasyCopy/releases/latest)；[中转服务 0.1.0 部署包](https://github.com/qmtdlt/EasyCopy/releases/tag/relay-v0.1.0)。

桌面安装包提供 macOS Apple Silicon / Intel DMG 和 Windows x64 NSIS。Mac 发布包必须完成 Developer ID 签名和 Apple 公证；Windows 首版未签名。首版不含自动更新，暂不提供 Linux 桌面客户端。

打包与验证步骤见 [macOS 发布](docs/macOS发布.md)、[Windows 发布](docs/Windows发布.md) 和 [中转服务发布](docs/中转服务发布.md)。桌面 `desktop-v*` 与中转 `relay-v*` 标签独立；手动 Actions 只验证，不公开 Release 或正式镜像。正式发布前请以对应文档的验收记录为准。

## 本机运行

需要 Rust 1.90+、Node.js 22+、Xcode Command Line Tools（macOS）。依赖版本固定于 Cargo.lock 和 package-lock.json。

```sh
./script/build_and_run.sh
```

该脚本默认构建并打开 **Release** 版 `target/release/bundle/macos/EasyCopy.app`，适合日常大文件传输。`--build` 只构建和签名，`--verify` 启动并检查进程，`--logs` 查看日志；`--debug` 才构建 Debug 版并进入调试器。Codex 的 Run 按钮也调用此脚本。关闭窗口后仍继续同步；从菜单栏托盘打开或退出。

Mac 运行脚本会为整个应用包签名，优先使用钥匙串中唯一的 Apple Development 证书；可通过 `EASYCOPY_SIGNING_IDENTITY` 指定证书名称或指纹。无唯一开发证书时退回完整临时签名并提示，局域网权限识别不保证同样稳定。应用包含局域网用途说明；首次出现系统询问时允许 EasyCopy 连接本地网络。

开发构建重新签名后，macOS 可能要求重新允许读取钥匙串。请在系统弹窗中处理；等待授权期间窗口、本地复制与历史仍可用。`--acceptance` 使用 `.local/desktop-acceptance` 隔离验收配置，不读取普通配置的凭据。

开发模式：

```sh
cd apps/desktop
npm ci
npm run tauri -- dev
```

浏览器 `http://127.0.0.1:1420` 只能查看界面，不能访问原生剪贴板或连接真实服务。

## 服务端

`192.168.1.48` 的离线部署、客户端配对及维护步骤见 [Ubuntu 部署记录](docs/Ubuntu部署记录.md)。

从 Windows PC 构建 Linux 镜像并部署到该服务器的规划见 [Windows 构建与部署方案](docs/Windows构建部署方案.md)；该方案尚待 Windows 实机验证。macOS 继续使用上述既有流程。

以下为可访问镜像仓库的 Linux Docker 主机上的常规构建方式：

```sh
docker compose -f deploy/compose.yaml up -d --build
docker compose -f deploy/compose.yaml exec relay easycopy-relay pairing
```

默认端口 8443。通过可信终端或 SSH 获取第二条命令输出的完整 `easycopy1:…` 配对串，在两端设置中填入服务器 IP、端口、连接密钥，先测试再保存。不要从不可信网页或首次连接自报证书中取得配对信息。

首启生成独立私有 CA、服务器证书及随机认证密钥，持久化于 Compose 卷。配对串只有公共 CA、逻辑服务器身份和认证信息，不含私钥。客户端只信任此 CA，按稳定逻辑名校验证书；实际 TCP 连接映射到设置中的地址。换 IP 不需要重新配对。完整部署、备份和故障说明见 [服务端实现](docs/服务端实现.md)。

本机独立服务（仅监听回环地址）：

```sh
cargo run --locked -p easycopy-relay -- serve --data-dir .local/relay --bind 127.0.0.1:8443
# 另一终端中取得配对串
cargo run --locked -p easycopy-relay -- pairing --data-dir .local/relay
```

## Windows 运行与构建

需要 Rust stable（MSVC 工具链）、Visual Studio C++ Build Tools（含 C++ x64 工具和 Windows SDK）、WebView2 Runtime 和 Node.js 22+。

双击仓库根目录的 `Start-EasyCopy.cmd`。首次自动安装 npm 依赖、构建并启动 **Release** 桌面客户端；之后直接打开 `.local/windows-client/EasyCopy.exe`，不依赖 Vite 或终端常驻。没有构建模式标记的旧 Debug 客户端会在退出后首次启动时自动重建为 Release。关闭主窗口后仍在后台运行，从系统托盘菜单打开或退出。若已在运行，脚本提示使用托盘，不重复启动。

更新代码后，在仓库根目录重新构建（先从托盘退出客户端）：

```powershell
.\Start-EasyCopy.cmd -Build
```

开发热更新用 `.\Start-EasyCopy.cmd -Dev`；隔离验收用 `.\Start-EasyCopy.cmd -Acceptance`（配置位于 `.local/windows-acceptance`）。需要带前端资源的 Debug 客户端时用 `-DebugBuild -Build`，输出到独立的 `.local/windows-debug-client/`。普通配置和历史位于 `%APPDATA%\com.easycopy.desktop`，连接密钥保存在 Windows 系统凭据中。不要手工把 `target/debug/EasyCopy.exe` 当作固定入口：开发模式也会覆盖这个文件，它可能需要 Vite。

连接时需填写 Windows 能访问的中转服务器 IP/域名、实际暴露端口和完整配对串。Mac 验收使用的 `127.0.0.1:58444` 是 Mac 本机地址，不能直接用于 Windows。配对串只在应用设置中输入。

如需生成安装包，在 Windows PowerShell 中：

```powershell
cd apps/desktop
npm ci
npm run tauri -- build --bundles nsis
```

安装包在 `target/release/bundle/nsis/`。此为构建流程；本轮是否完成 Windows 实机验证，以 [测试记录](docs/测试记录.md) 为准。

## 使用规则

- 自动保留最近 10 条，按复制意图顺序展示。文本、图片可再次复制或另存为，再次复制会创建新的同步意图；文件与文件夹仅支持另存为。
- 本机复制文件时，按本次所选文件及文件夹内部普通文件的总大小判断是否询问上传。设置页可修改“文件上传确认”阈值，默认 50 MB（1 MB = 1,000,000 字节），支持正整数，保存后从下一次复制生效。严格超过阈值才询问，默认选“否”；按 Esc、关闭弹窗或主窗口也视为拒绝，不影响本机粘贴，不登记任务、不打包，重连或重启也不补发。
- 等待确认时复制新内容，会取消旧询问；重新复制同一文件会重新判断。远端接收不弹上传询问。图片内容不受文件阈值限制，图片文件按文件处理。
- 图片内容和图片文件分别处理；文件粘贴得到原文件/目录，不变成 ZIP，也不引用另一台电脑的路径。
- 离线复制只保留本地历史。重连只补历史，不自动覆盖当前剪贴板；已经登记的中断任务沿原身份从头重试，最多 3 次自动重试。
- 当前剪贴板和在途任务引用的缓存持续保留；无引用缓存延迟 24 小时清理。另存为后的用户文件不受缓存回收影响。
- 只支持普通文件与目录。符号链接、特殊文件、跨平台名称冲突明确报错；不移动或删除源文件。
- 自身 `easycopy1:` 连接密钥不进入历史或同步队列；设置页将它保存在系统凭据存储。
- 传输提示在当前显示器工作区右下角：上传向上填充、下载向下填充。准备阶段不显示假进度，失败会区别于成功。
- 不同应用接受的剪贴板格式有差别。微信、Finder/资源管理器的实测覆盖见测试记录，不承诺所有应用都接受文件夹或应用私有格式。

## 验证

大文件分阶段基准、隔离局域网测量及 Windows 实机验收边界见 [大文件传输性能](docs/大文件传输性能.md)。

```sh
cargo test --locked -p easycopy-protocol -p easycopy-client-core -p easycopy-relay -p easycopy-integration-tests
cd apps/desktop
npm test
npm run build
```

真实原生剪贴板探针默认忽略，单独运行前应在受控环境保存当前工作：

```sh
cargo test -p easycopy-client-core --test native_clipboard -- --ignored --test-threads=1
```

它只恢复本产品支持的文本、图片、文件格式，不能保真恢复第三方应用私有剪贴板数据。其他自动测试使用隔离内存剪贴板和临时服务目录。

## 目录

- `crates/protocol`：共享协议与配对串。
- `crates/client-core`：原生剪贴板、同步、历史、缓存、安全归档。
- `crates/relay`：HTTPS/WSS、SQLite 元数据、磁盘载荷。
- `crates/integration-tests`：真实网络双客户端联调。
- `apps/desktop`：定稿 Vue 界面、Tauri 桌面入口与浮层。
- `deploy`：Linux Docker Compose 部署包。
- `design`：保留的用户定稿与模拟演示，不属于正式客户端。

实现参考：[reqwest 客户端 TLS/地址映射](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html)、[WSS 自定义 TLS 连接](https://docs.rs/tokio-tungstenite/0.28.0/tokio_tungstenite/fn.client_async_tls_with_config.html)、[Windows 剪贴板所有者规则](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-openclipboard)。
