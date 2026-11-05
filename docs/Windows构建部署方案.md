# Windows 构建与部署中转服务方案

状态：方案，尚未在 Windows 实机执行。用户后续要求构建或部署时，按本方案检查环境、完成所需准备并执行；本文不表示脚本已经实现或服务已经更新。

## 目标与分工

在 Intel Windows PC 上构建 `linux/amd64` 中转服务成品镜像，通过局域网传到 Ubuntu，由 Ubuntu 加载并启动。Ubuntu 不编译、不下载依赖。首次构建可能需要下载基础镜像和 Cargo 依赖；后续复用 Windows 上的缓存。重启现有服务不需要重新构建。

本方案针对现有服务器 `qmtdlt@192.168.1.48`、目录 `/home/qmtdlt/easycopy`、Compose 项目 `easycopy` 和数据卷 `easycopy_relay-data`。执行前重新确认地址、架构、权限和现状；不要把示例地址当成对其他服务器的授权。

## 环境准备

- Windows 使用 Docker Desktop 的 **Linux 容器**，确保 Docker 引擎可用、可构建 `linux/amd64`；检查可用磁盘和内存。无需为服务端构建安装 Windows Rust 或 Visual Studio 工具链。
- Windows 可访问所需镜像仓库及 Cargo 依赖源；首次拉取、缓存缺失或依赖更新时需要网络。如果网络不可用或缺包，停止并说明，不让 Ubuntu 临时转为在线构建。
- Windows 安装 SSH/SCP 客户端，能通过可信方式验证服务器主机身份并登录；不要假设 Mac 上的 SSH 密钥已存在于 Windows，也不要关闭主机密钥校验。
- Ubuntu 确认为 `x86_64`、Docker 与 Compose 可用；当前 Snap Docker 主机仍同时使用 `deploy/compose.yaml` 和 `deploy/compose.snap.yaml`。普通 Docker 主机不自动使用该覆盖文件。
- 构建前核对 Git 版本及未提交修改，明确成品对应的实际源码；不得把包含未提交修改的成品只标为某个干净提交。不得覆盖或清理用户现有修改。

## 构建方案

1. 在 Windows 的项目工作区准备 Linux 构建环境，以 `linux/amd64` 构建 `easycopy-relay` 的 Release 版本，使用 `Cargo.lock` 和 `cargo build --locked --release -p easycopy-relay`。优先采用与当前 Ubuntu 已验证路径一致的 Alpine/musl 静态链接方式，固定并记录实际 Rust 基础镜像版本及镜像 ID；确认所用工具链满足当前依赖要求。
2. 通过 Docker 构建缓存或持久化 Cargo 缓存复用依赖和中间产物；不要将缓存、密钥或本地数据打进最终镜像。源码变化时确保对应层重新构建，不得因缓存误用旧程序。
3. 检查产物确为 `x86_64` Linux 静态链接程序。按现有 [Dockerfile.offline](../deploy/Dockerfile.offline) 的约定封装 `scratch` 运行镜像：UID/GID `10001:10001`、`/data`、健康检查及启动参数保持一致。构建过程可能使用网络，最终镜像和 Ubuntu 加载过程不依赖仓库。
4. 给镜像加唯一版本标签，同时记录实际源码状态、构建时间、平台、基础镜像及最终镜像 ID。导出镜像包并计算 SHA-256；镜像包不包含数据卷、配对密钥或服务器私钥。导出包放在不纳入 Git 的工作目录，完成后说明保留位置。

当前仓库没有实现上述 Windows 服务端构建脚本。实际首次执行时应先选择并准备可复用的构建命令或脚本，检查 Docker Desktop 的文件挂载、权限和缓存行为；不要假定仓库现有 `deploy/Dockerfile` 已满足本方案，它目前使用 Debian 及不同的构建路径。

## 更新部署方案

仅在用户要求部署时执行；如果只要求构建，停在镜像导出和验证。

1. 检查 Ubuntu 当前服务健康状态、Compose 配置、正在运行的镜像 ID、数据卷与可用空间。确认部署可能造成短暂断线，并检查版本是否存在需要单独处理的数据迁移；记录旧镜像的不可变 ID 或保留标签，不能只依赖会被覆盖的 `latest`。
2. 通过 SSH/SCP 传输成品镜像包及校验值，在 Ubuntu 校验 SHA-256 后加载；核对镜像的 `linux/amd64` 架构和 ID。传输或校验失败时停止，不重建服务。正常更新不需要传源码或构建缓存。
3. 核对服务器上的两个 Compose 文件与预期配置一致；若本次版本需要变更配置，先检查差异再同步，保留原配置。当前 Compose 未显式指定 `image`，以项目名和服务名生成 `easycopy-relay:latest`；确认实际解析结果后，给新镜像打该标签，并使用两个 Compose 文件运行 `up -d --no-build --pull never`。不要运行普通的 `up --build`。
4. 检查容器状态、健康检查、近期日志，以及原数据卷仍被挂载；按本次变更范围确认服务功能和客户端连接。不要输出配对串、私钥或其他凭据。记录镜像 ID、源码状态、校验和、部署结果及未做的验证。
5. 若启动或检查失败，保留日志和旧镜像，根据故障情况使用旧镜像恢复；切换前先判断新版本是否改变数据格式，不能假设旧程序一定能读取新数据。恢复后再次检查健康状态并报告。

保留原有 `easycopy_relay-data`，不得执行 `docker compose down -v` 或删除数据卷。镜像回退不是数据备份；涉及数据迁移时，先评估并准备可恢复的数据备份。数据备份含私钥和认证信息，不能放入 Git 或镜像包。

## 与 Mac 流程的关系

后续按发出指令时**实际执行环境**选择：macOS 沿用 [Ubuntu 部署记录](Ubuntu部署记录.md)里的既有离线编译流程；Windows 使用本文方案。Windows 方案首次实机执行后，应补充实际命令、耗时、验证结果和遇到的问题；在此之前不承诺加速比例或声称该方案已验证。
