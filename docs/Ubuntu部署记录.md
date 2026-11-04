# Ubuntu 中转服务部署记录

部署日期：2026-11-03（Asia/Shanghai）。

## 目标与目录

- SSH：`ssh qmtdlt@192.168.1.48`，已配置公钥登录，无需输入密码。
- 系统：Ubuntu 22.04.5 LTS，x86_64；Docker 29.8.0，Compose v5.5.1。
- 服务地址：`192.168.1.48:8443`，HTTPS / WSS。
- 服务器目录：`/home/qmtdlt/easycopy`。
- Compose 项目：`easycopy`；服务：`relay`；数据卷：`easycopy_relay-data`。
- 使用 `deploy/compose.yaml` 与主机兼容覆盖 `deploy/compose.snap.yaml`：自动重启、只读根文件系统、独立数据卷、日志轮转。

## 为什么使用离线部署

首次执行普通 Compose 构建时，Ubuntu 访问 Docker Hub 的认证接口超时。按照用户要求，停止下载，改为通过局域网 SSH 传输本机已经存在的文件和镜像。

实际构建使用本机已有的 **linux/amd64 `rust:alpine`（Rust 1.99.1）** 和 Cargo 缓存。构建容器配置 `--network none`，Cargo 使用 `--offline --locked`。运行镜像使用 `FROM scratch`，没有需要从 Docker Hub 下载的基础镜像。

这条路径与原 `deploy/Dockerfile` 的 Rust 1.90 / Debian 构建路径不同。重新使用普通 `docker compose up --build` 会走回需要网络的路径；本机应使用以下离线流程或直接加载成品镜像。

## Snap Docker 启动兼容

首次启动被 AppArmor 拒绝，容器日志为 `exec /usr/local/bin/easycopy-relay: operation not permitted`。内核日志明确记录 `profile="snap.docker.dockerd"`、`target="docker-default"`、`info="no new privs"`。

这台主机使用 `deploy/compose.snap.yaml` 清空该服务的 `security_opt`，仅取消与 Snap 冲突的 `no-new-privileges` 启动限制。实际检查确认仍为 UID/GID `10001:10001`、`ReadonlyRootfs=true`、`CapDrop=["ALL"]`、`AppArmorProfile=docker-default`，没有使用 privileged、host 网络或关闭 AppArmor。Docker 默认 seccomp 仍生效。

本机所有创建、更新、重建命令都必须同时传入两个 Compose 文件。其他不受影响的 Docker 主机继续使用原配置，不需要这个覆盖文件。`!override` 要求支持该语法的 Compose；本机 v5.5.1 已验证。

## 首次部署步骤

以下 Mac 命令在项目根目录执行。前提是本机已经缓存 amd64 构建镜像及所需 Cargo 依赖；其他电脑不应假设也有这些缓存。

### 1. 复制源码

```sh
set -o pipefail
tar --no-xattrs -czf - .dockerignore Cargo.toml Cargo.lock crates apps/desktop/src-tauri deploy |
  ssh qmtdlt@192.168.1.48 'umask 077; mkdir -p /home/qmtdlt/easycopy; tar -xzf - -C /home/qmtdlt/easycopy'
```

不复制 Mac 的 `target`、客户端凭据、测试数据或本机服务身份。

### 2. 传输已有构建镜像和依赖

```sh
set -o pipefail
docker image save --platform linux/amd64 rust:alpine | gzip -1 |
  ssh qmtdlt@192.168.1.48 'docker image load'

tar --no-xattrs -czf - -C "$HOME/.cargo" registry/cache registry/index |
  ssh qmtdlt@192.168.1.48 'umask 077; mkdir -p /home/qmtdlt/easycopy/.build/cargo; tar -xzf - -C /home/qmtdlt/easycopy/.build/cargo'
```

### 3. 在 Ubuntu 断网编译

```sh
ssh qmtdlt@192.168.1.48
cd /home/qmtdlt/easycopy

docker run --rm --pull never --network none \
  --name easycopy-offline-builder --cpus 3 --memory 4g \
  -e CARGO_HOME=/cargo \
  -e CARGO_TARGET_DIR=/build/.build/target \
  -e CARGO_BUILD_JOBS=3 \
  -v /home/qmtdlt/easycopy:/build \
  -v /home/qmtdlt/easycopy/.build/cargo:/cargo \
  -w /build rust:alpine \
  cargo build --offline --locked --release -p easycopy-relay \
  > deployment-offline-build.log 2>&1
```

保留 `.build` 供后续增量构建。若更新依赖后离线构建提示缺包，先在开发机准备对应缓存，再传输；不要默默改为在线构建。

### 4. 封装并启动

```sh
cd /home/qmtdlt/easycopy
mkdir -p .build/runtime/data
chmod 700 .build/runtime/data
cp .build/target/release/easycopy-relay .build/runtime/easycopy-relay
chmod 755 .build/runtime/easycopy-relay
file .build/runtime/easycopy-relay

docker build --network none --pull=false \
  -f deploy/Dockerfile.offline -t easycopy-relay:latest .build/runtime
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml up -d --no-build --pull never
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml ps
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml exec -T relay easycopy-relay healthcheck
```

`file` 应确认程序为 x86-64 Linux 静态链接文件。`Dockerfile.offline` 仅适用于这样的二进制文件，不能放入 macOS 程序或依赖动态库的 Linux 程序。

## 客户端配对

两端客户端填写：服务器 `192.168.1.48`，端口 `8443`。通过可信 SSH 获取完整连接密钥：

```sh
ssh qmtdlt@192.168.1.48 \
  'cd /home/qmtdlt/easycopy && docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml exec -T relay easycopy-relay pairing'
```

将输出的整串 `easycopy1:…` 填入客户端，测试连接后保存。密钥不写入本记录或 Git。不要用浏览器直接访问 IP 时的自签名证书警告判断服务是否故障；客户端使用配对串内的私有 CA 和逻辑名称校验 TLS。

## 日常操作

在 Ubuntu 的 `/home/qmtdlt/easycopy` 中执行：

```sh
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml ps
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml logs --tail 100 relay
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml restart relay
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml exec -T relay easycopy-relay healthcheck
```

不要执行 `docker compose down -v` 或删除 `easycopy_relay-data`。数据卷同时保存服务器身份、密钥、SQLite 元数据及载荷；丢失身份会导致原客户端配对失效。

运行镜像不包含 shell、curl 或 tar，排查优先使用上述程序自检、容器日志及宿主机工具。

2026-11-03 已部署镜像保留全部已完成数据，没有自动清理；剩余空间接近 1 GiB 时会拒收新内容。

2026-11-04 已部署提交 `4dc0b98`，按登记序号保留最近 10 条记录对应文件，详见[服务端实现](服务端实现.md)。启动后检查到 10 个载荷文件、0 个临时文件，载荷目录约 61.3 MiB；容器健康检查通过。更新前曾备份完整数据卷并通过 gzip 校验；备份已于同日按用户要求删除。旧镜像保留为 `easycopy-relay:before-4dc0b98`，新镜像为 `easycopy-relay:4dc0b98`。未执行恢复演练或客户端端到端测试。活跃上传/下载期间允许暂时超过 10 条，传输结束后回收。

## 验收与归档

2026-11-03 23:43（Asia/Shanghai）部署完成，容器 `easycopy-relay-1` 为 `healthy`，重启策略为 `unless-stopped`。Docker 系统服务已启用开机启动；未重启整台 Ubuntu 验证开机流程。

本次通过的部署验收：

- 在 Mac 上通过真实局域网地址访问，私有 CA、TLS 主机名及服务器身份校验通过；携带认证返回 200，缺少认证返回 401。
- WSS 握手成功，收到历史快照。
- 登记并上传 55 字节中文文本，收到 WSS 完成事件，下载内容及 SHA-256 一致。
- 强制重建容器后重新检查：服务器身份未变，原记录及载荷仍可读取，容器健康检查通过。
- 其他已有容器保持运行；未修改宿主机 Docker 配置、代理、AppArmor 策略或防火墙。

服务中留有一条标记为“部署验证”的文本记录。本次未执行桌面客户端完整联调、Windows 实机验证、大文件压力测试或全量自动测试。

构建耗时 8 分 14 秒；程序为 `x86-64 static-pie`。运行镜像 ID：

```text
sha256:4d65b50d0b5cf634d7309946cf2c4726f5bfb52540adcd1a9851b2ae6426b177
```

镜像同时标记为 `easycopy-relay:latest` 和 `easycopy-relay:2026-11-03`。成品压缩包约 4.7 MiB：

- Ubuntu：`/home/qmtdlt/easycopy/releases/easycopy-relay-linux-amd64-2026-11-03.tar.gz`。
- Mac：项目内 `.local/ubuntu-release/easycopy-relay-linux-amd64-2026-11-03.tar.gz`。
- SHA-256：`f621cde2f414d3aa4728a0432d8bdbe55d60a9379a0661d2c93729bc6d8d74eb`，两端核对一致。

服务器另保存 `deployment-source.sha256`（源码文件校验清单）、`deployment-offline-build.log`（成功构建日志）及 `deployment-build.log`（首次在线构建失败日志）。当时源码尚未提交 Git，不能把这次部署标记为某个已提交版本。

### 后续直接使用成品镜像

在当前 Ubuntu 上重新加载镜像并启动：

```sh
cd /home/qmtdlt/easycopy
sha256sum -c releases/SHA256SUMS
docker image load -i releases/easycopy-relay-linux-amd64-2026-11-03.tar.gz
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml up -d --no-build --pull never
```

也可从 Mac 用 `scp` 传输成品包和 `deploy` 目录到另一台 x86_64 Linux Docker 主机，再执行加载和启动。镜像包不含运行中的配对密钥或用户数据；迁移现有服务器身份时还必须另行恢复完整数据卷。普通 Docker 主机不应自动套用 Snap 覆盖文件。

### 初始数据备份

首次部署时在停止服务后通过 `docker cp` 导出整个数据目录，原先保存为以下文件；该备份已于 2026-11-04 按用户要求删除：

```text
/home/qmtdlt/easycopy/backups/initial-data-2026-11-03.tar.gz
```

该备份包含完整 `identity/`、SQLite 文件和验证载荷。目前尚未建立异机或自动备份。数据备份包含私钥和认证信息，不放入镜像包或 Git。

后续需要备份时，在服务目录执行以下命令；停止期间客户端会短暂断线：

```sh
set -euo pipefail
umask 077
mkdir -p backups
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml stop relay
trap 'docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml start relay' EXIT
docker cp easycopy-relay-1:/data/. - | gzip -1 > "backups/data-$(date +%Y%m%d-%H%M%S).tar.gz"
docker compose -f deploy/compose.yaml -f deploy/compose.snap.yaml start relay
trap - EXIT
```

服务端当前停止时可能等待约 20 秒后才退出；应等停止命令结束再复制，完整保留 SQLite 的 WAL 等文件。恢复备份未做演练；本次验证的是同一数据卷在容器重建后继续可用。
