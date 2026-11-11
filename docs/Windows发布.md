# Windows x64 安装包

面向 Intel / AMD x64，产物为**未签名 NSIS `.exe`**。不构建 ARM64、MSI、Linux 桌面，不配置 Windows 签名证书，不含自动更新。用户安装或下载时可能看到“未知发布者”或 SmartScreen 提示。

## 构建和产物

工作流：`.github/workflows/package-windows.yml`。支持 `workflow_dispatch` 手动验证和 `workflow_call` 被桌面统一发布流程调用；自身只有 `contents: read` 权限，不创建标签、Release 或发布镜像。

- Windows Server 2022 GitHub runner、Node.js 22、Rust stable MSVC，目标固定 `x86_64-pc-windows-msvc`。
- `npm ci` 使用 `apps/desktop/package-lock.json`；Tauri release 构建传递 Cargo `--locked` 使用根 `Cargo.lock`。
- Tauri 在 Windows 自动合并 `tauri.windows.conf.json`，只构建 NSIS。
- 可选输入 `version` 仅检查预期版本，不改版本文件。当前脚本接受稳定版 `x.y.z`；版本及 `desktop-v*` 标签由统一桌面流程协调。
- 安装产物 artifact：`windows-x64`，扁平包含 `EasyCopy_<version>_windows_x64_setup.exe` 和同名 `.exe.sha256`。
- 检查报告单独放在 `windows-x64-verification`，不混入安装产物。Actions 保存 14 天；正式版本的统一 `SHA256SUMS` 由桌面发布流程生成。

在 GitHub Actions 中手动运行 **Package Windows**，选择已推送的分支，可留空 `version`。CLI 等效命令：

```sh
gh workflow run package-windows.yml --repo qmtdlt/EasyCopy --ref main
```

本地 Windows 构建需 PowerShell 7、Node.js 22、Rust MSVC 和 Visual Studio C++ Build Tools / Windows SDK：

```powershell
pwsh -File script/package_windows.ps1
```

输出在 `.local/packages/windows-x64/`。此脚本不安装应用；现有 `script/build_and_run.ps1` 保持原样。

## 安装、升级与数据

固定应用名 `EasyCopy`、程序名 `EasyCopy.exe`、标识 `com.easycopy.desktop`。当前用户安装，默认 `%LOCALAPPDATA%\EasyCopy`，不主动要求管理员权限。安装器允许选择目录，并记住已有安装位置。开始菜单生成 `EasyCopy` 快捷方式，交互安装可在结束页选择桌面快捷方式；静默安装创建两处快捷方式。

WebView2 策略为 `downloadBootstrapper`：已有运行时则复用；缺少时联网下载微软引导程序并静默安装。**这不是离线安装包**。CI runner 通常已有 WebView2，不能用 runner 上安装成功证明“无 WebView2 的干净系统”路径已通过。

用户数据继续由应用保存于 `%APPDATA%\com.easycopy.desktop`，配对秘密继续使用现有 Windows 系统凭据存储。安装资源不添加外部文件或 sidecar，只打包程序、图标及前端运行资源；不添加仓库文档、测试目录、开发配置、配对凭据或开发机数据路径。`public/assets/approved-reference.png` 是 `transfer-art.ts` 使用的运行素材，需保留。

升级方式是退出托盘中的 EasyCopy 后运行新安装包，沿用原安装目录；无自动更新。安装过程不主动改写用户数据或凭据。禁用静默降级；交互安装遇到较新版本时可能要求先卸载，不能视为数据迁移方案。卸载默认保留用户数据，交互卸载若勾选删除应用数据则例外。

自启动仍由应用设置控制（HKCU Run 的 `com.easycopy.desktop` 项），安装器不默认开启。现有启动项包含安装路径；切换安装目录或从开发脚本版迁移后，应在新应用内关闭再开启自启动。卸载前应关闭自启动，避免遗留应用自行创建的 Run 项；安装器不访问或删除系统凭据。

## 验证范围

`script/verify_windows_package.ps1` 只允许一次性 GitHub-hosted Windows runner，检测到已有安装或数据即拒绝运行。不要在日常使用的 Windows 用户下强行绕过保护。

自动检查内容：安装包及应用未签名、静默安装退出码、默认用户安装目录、AMD64 PE 架构、应用和注册表版本、开始菜单与桌面快捷方式指向、同版本覆盖安装、数据哨兵保留、静默卸载及卸载注册项移除。哨兵是 CI 生成的无敏感内容文件，不是用户真实配置；这些检查不证明设置迁移和凭据读取正确。

截至本次实现，Windows CI 尚待首次运行；本地执行环境是 macOS，不能把配置检查视为 Windows 实测。后续运行结果在本节追加。

正式对外提供安装包前仍需真实 Windows 10/11 x64 验收：

1. 无 WebView2 的干净用户首次安装，以及已有运行时的安装。
2. 与 Mac 双向文本、图片、文件剪贴板同步，重启后配置和凭据读取。
3. 自启动开关、注销后登录、系统启动应用禁用状态。
4. 不同版本覆盖升级后设置、历史和凭据保留；中文/空格路径安装。
5. 交互卸载、快捷方式清理和 SmartScreen 提示流程。

配置依据：[Tauri Windows 安装器](https://v2.tauri.app/distribute/windows-installer/) 与锁定 CLI `2.12.4` 的 [NSIS 模板](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.4/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi)。
