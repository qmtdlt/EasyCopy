# macOS 签名、公证与桌面发布

## 产物和边界

面向 macOS 12+，分别构建 Apple Silicon `arm64` 和 Intel `x64` 的 DMG。应用标识仍为 `com.easycopy.desktop`，沿用现有权限、局域网用途说明和客户端行为。DMG 内只有 `EasyCopy.app` 与指向 `/Applications` 的快捷方式，拖入“应用程序”即可安装。首版不含自动更新，不提供 Linux 桌面客户端。

- Mac：`.github/workflows/package-macos.yml` 支持 `workflow_call` 和 `workflow_dispatch`，始终要求签名公证，无未签名回退。
- Windows：`.github/workflows/package-windows.yml` 提供同样的调用入口，见 [Windows 发布](Windows发布.md)。
- 桌面统一入口：`.github/workflows/release-desktop.yml`。手动运行只构建验证；仅推送 `desktop-v*` 标签才会公开 Release。
- 中转服务使用独立 `relay-v*` 标签，见 [中转服务发布](中转服务发布.md)。

三个平台成功后，统一入口验证每个包的 SHA-256，生成 `SHA256SUMS`，由唯一 job 创建同一个 GitHub Release。任意构建或校验失败，都不会发布不完整的桌面版本。不要在单个平台流程中单独创建 Release。

正式附件保留带版本号的三个安装包，同时提供 `EasyCopy_macos_arm64.dmg`、`EasyCopy_macos_x64.dmg`、`EasyCopy_windows_x64_setup.exe` 三个内容相同的固定名称副本，全部纳入 `SHA256SUMS`。官网通过 `https://github.com/qmtdlt/EasyCopy/releases/latest/download/固定文件名` 下载最新正式桌面版。流水线先上传到草稿 Release，再下载全部附件逐字节比较并校验 SHA-256，通过后公开并设为 Latest；失败时保留草稿供检查，不覆盖已有 Release。

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

2026-11-11：四份 Actions 工作流通过 actionlint 1.7.12；Mac Python 脚本语法、版本一致性、错误标签拒绝、缺凭据提前停止与空清理检查通过。本机 Apple Silicon 上分别完成 ARM64 原生构建和 Intel 交叉构建，两个 `.app` 都通过架构、应用标识、最低系统版本、局域网用途说明、Developer ID、时间戳、hardened runtime 及嵌套签名验证。ARM64 临时 DMG 的创建、Developer ID 签名、镜像完整性、只读挂载及包内容检查也通过，临时镜像已删除。

已为 EasyCopy 单独配置证书 Secrets 和签名变量，维护者直接填写了 Apple 公证账号与专用密码。[桌面统一手动运行 42945384612](https://github.com/qmtdlt/EasyCopy/actions/runs/42945384612) 全部成功，源码提交为 `6ffff329602de642887b9c806b7bfe90d6cc4e31`。ARM64 和 Intel 都在对应原生 runner 完成构建、Developer ID 签名、公证、票据附加、镜像/载荷检查及 DMG/内部应用的 Gatekeeper 验证，结果均为 `Notarized Developer ID`（验证通过）。两端临时钥匙串清理成功。

| 架构 | Apple 公证提交 ID | 结果 | Actions artifact |
| --- | --- | --- | --- |
| Apple Silicon | `966abe32-629d-4751-9f32-442ea3cd83ce` | Accepted | [macos-arm64](https://github.com/qmtdlt/EasyCopy/actions/runs/42945384612/artifacts/13821729140) |
| Intel | `65d487fe-bfb6-4702-8114-6cb06b5427b5` | Accepted | [macos-x64](https://github.com/qmtdlt/EasyCopy/actions/runs/42945384612/artifacts/13822553491) |

同一次运行的 Windows 安装验证也成功，汇总 job 校验三个包的 sidecar 后生成统一 `SHA256SUMS`，产出 [desktop-release-candidate](https://github.com/qmtdlt/EasyCopy/actions/runs/42945384612/artifacts/13822104867)。公开 Release 步骤为 `skipped`，没有创建正式标签、Release 或 GHCR 镜像，也没有部署服务器。Actions artifacts 保留 14 天。

两个最终 DMG 均已下载到本机 Apple Silicon Mac，并独立通过 SHA-256、Developer ID、票据、Gatekeeper、镜像及挂载应用复核。文件分别位于 `dist/macos/arm64/EasyCopy_0.1.0_macos_arm64.dmg` 与 `dist/macos/x64/EasyCopy_0.1.0_macos_x64.dmg`，同目录包含对应 `.sha256`；产物未纳入 Git。该复核没有安装或启动应用。

尚未执行 Apple Silicon/Intel 实机安装、首次启动、权限授权、既有配置升级和真实跨设备剪贴板同步验收。Windows 仍未 Authenticode 签名。正式标签触发及公开 Release 上传尚未执行，不把手动验证等同于正式发布。

官方依据：[Tauri macOS 签名](https://v2.tauri.app/distribute/sign/macos/)、[Apple 公证工作流](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)。

## 首次正式发布记录

2026-11-11 经用户授权，推送 `desktop-v0.1.0`，源码提交 `c5c21aabda3f87427bd00ef593237c26decb624d`。[正式运行 42949212409](https://github.com/qmtdlt/EasyCopy/actions/runs/42949212409) 全部通过：两个 Mac 原生架构分别重新构建、签名、公证并校验，Windows 构建及安装验证通过，统一汇总核验全部附件后公开 [EasyCopy v0.1.0](https://github.com/qmtdlt/EasyCopy/releases/tag/desktop-v0.1.0) 并设为 Latest。

Release 包含三个带版本号的安装包、三个内容相同的固定下载名称副本及 `SHA256SUMS`，共七个附件。公开前已从草稿下载全部附件，与构建产物逐字节比较并验证六个包的 SHA-256；公开后再次匿名下载六个包校验通过，三个 Latest 固定下载入口返回的内容也与对应版本包完全一致。Windows 未签名、无自动更新和需自建中转服务均已写入发布说明；本次未新增真实设备手动安装、升级与跨设备剪贴板验收。
