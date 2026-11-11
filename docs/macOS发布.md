# macOS 签名、公证与桌面发布

## 产物和边界

面向 macOS 12+，分别构建 Apple Silicon `arm64` 和 Intel `x64` 的 DMG。应用标识仍为 `com.easycopy.desktop`，沿用现有权限、局域网用途说明和客户端行为。DMG 内只有 `EasyCopy.app` 与指向 `/Applications` 的快捷方式，拖入“应用程序”即可安装。首版不含自动更新，不提供 Linux 桌面客户端。

- Mac：`.github/workflows/package-macos.yml` 支持 `workflow_call` 和 `workflow_dispatch`，始终要求签名公证，无未签名回退。
- Windows：`.github/workflows/package-windows.yml` 提供同样的调用入口，见 [Windows 发布](Windows发布.md)。
- 桌面统一入口：`.github/workflows/release-desktop.yml`。手动运行只构建验证；仅推送 `desktop-v*` 标签才会公开 Release。
- 中转服务使用独立 `relay-v*` 标签，见 [中转服务发布](中转服务发布.md)。

三个平台成功后，统一入口验证每个包的 SHA-256，生成 `SHA256SUMS`，由唯一 job 创建同一个 GitHub Release。任意构建或校验失败，都不会发布不完整的桌面版本。不要在单个平台流程中单独创建 Release。

## 仓库凭据

在 EasyCopy 仓库 Settings → Secrets and variables → Actions 配置；其他仓库的 Secrets 不能读回或自动继承。

| 类型 | 名称 | 内容 |
| --- | --- | --- |
| Secret | `EASYCOPY_APPLE_CERTIFICATES_P12` | 仅含 Developer ID Application 证书及对应私钥的加密 P12，经 base64 编码 |
| Secret | `EASYCOPY_APPLE_CERTIFICATES_PASSWORD` | 上述 P12 的密码 |
| Secret | `EASYCOPY_APPLE_ID` | Apple 开发者账号 |
| Secret | `EASYCOPY_APPLE_APP_PASSWORD` | Apple 账号生成的专用密码，不是网页登录密码 |
| Variable | `EASYCOPY_APPLE_TEAM_ID` | 签名证书所属开发者 Team ID |
| Variable | `EASYCOPY_CODESIGN_IDENTITY` | Developer ID Application 完整名称或 SHA-1 指纹 |

同一开发者团队的有效 Developer ID Application 证书可以复用，无需为每个应用重签发。DMG 不需要 Developer ID Installer 证书。私钥/P12/密码不得写入代码、日志或安装包；不要把密码发送到聊天。

CI 在 `RUNNER_TEMP` 建立临时钥匙串，导入证书并存入公证 profile。P12 导入后立即删除；`always()` 清理步骤恢复钥匙串搜索列表并删除临时钥匙串，即使构建、公证或上传失败也执行。临时 runner 最终销毁。凭据不放进缓存或 Actions artifacts。

## 手动验证

```sh
gh workflow run package-macos.yml --ref main
# 或一次验证两套桌面流程，仍不会发布：
gh workflow run release-desktop.yml --ref main
```

每个架构在对应原生 GitHub runner 上构建：`macos-15`（ARM64）、`macos-15-intel`（Intel）。流程执行：

1. 核对 Cargo workspace、桌面 package.json、package-lock.json 和 Tauri 版本；标签调用还要求标签等于 `desktop-v<version>`。
2. 校验证书、公证凭据，安装固定依赖，运行前端测试与构建。
3. Tauri 按由内到外顺序用 Developer ID Application 签名应用和嵌套代码，启用 hardened runtime 与安全时间戳。
4. 检查架构、签名团队、时间戳、权限说明，生成只读 DMG 并签名。
5. 每次新构建都经 `notarytool submit --wait`，明确取得 `Accepted`；附加并验证 Apple 票据。
6. 运行 `codesign`、`hdiutil verify`、`stapler validate` 及 DMG/挂载应用的 Gatekeeper 校验。全部通过后才写入最终路径并计算 SHA-256。

Artifacts 保留 14 天，文件扁平存放：

- `macos-arm64`：`EasyCopy_<version>_macos_arm64.dmg` 及 `.dmg.sha256`。
- `macos-x64`：`EasyCopy_<version>_macos_x64.dmg` 及 `.dmg.sha256`。
- 统一入口另外生成 `desktop-release-candidate`，包含三个最终安装包与统一 `SHA256SUMS`。

本地已有证书及 `notarytool` profile 时，也可运行以下命令。需先安装 Node.js 22+、Rust、对应 target、Python 3.11+ 与 Xcode Command Line Tools，并在 `apps/desktop` 执行 `npm ci`。

```sh
export EASYCOPY_APPLE_TEAM_ID='<Team ID>'
export EASYCOPY_CODESIGN_IDENTITY='Developer ID Application: <姓名> (<Team ID>)'
python3 script/package-macos.py --arch arm64 --notary-profile '<钥匙串中的 profile>'
python3 script/verify-macos-package.py --arch arm64
```

Intel 目标使用 `--arch x64`，本地交叉构建不代表已在 Intel 机器上运行过。自定义钥匙串可传 `--keychain`。

## 后续正式发布

仅在用户明确授权正式发布后操作。先同步所有版本字段（含 Cargo.lock 中本项目 package 条目与 package-lock.json 根 package），确认 `main` 已推送，再创建并推送 `desktop-v<version>`。本次实现和验收不创建正式标签。

标签触发统一入口，两套工作流都成功后公开三个包。发布说明明确 Windows 未 Authenticode 签名、首版无自动更新及需自行部署中转服务。已存在的同名 Release 不会被静默覆盖。

## 验收记录

2026-11-11：正在建立并验证打包链路。实际 CI 运行结果将在验证后补充；在有成功证据前不得声称两架构签名公证完成。CI 中的包校验不能代替 Apple Silicon/Intel 实机安装、首次启动、权限授权、既有配置升级和真实跨设备剪贴板同步验收。

官方依据：[Tauri macOS 签名](https://v2.tauri.app/distribute/sign/macos/)、[Apple 公证工作流](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)。
