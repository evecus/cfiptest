# cfiptest

Cloudflare 优选 IP 自动测速 & DNS 同步工具。

- 从社区维护的精选 IP 列表拉取候选 IP
- 并发测 TCP 443 延迟，取最低 N 个测速
- Top 3 IP 自动同步到 Cloudflare DNS
- 任务完成后自动重启进程释放内存
- 内置 cron 定时，无需外部调度器

---

## 环境变量

| 变量 | 必填 | 说明 | 默认值 |
|------|------|------|--------|
| `CF_API_TOKEN` | ✅ | Cloudflare API Token（需要 Zone DNS Edit 权限） | — |
| `CF_ZONE_ID` | ✅ | Cloudflare Zone ID | — |
| `CF_DOMAIN_1` | ✅ | 第 1 个完整域名，如 `cf1.yourdomain.com` | — |
| `CF_DOMAIN_2` | ✅ | 第 2 个完整域名，如 `cf2.yourdomain.com` | — |
| `CF_DOMAIN_3` | ✅ | 第 3 个完整域名，如 `cf3.yourdomain.com` | — |
| `CF_CRON` | | cron 表达式 | `0 3 * * *` |
| `CF_IP_LIST_URL` | | 优选 IP 列表 URL | ymyuuu/Cloudflare-Better-IP |
| `CF_TEST_CONCURRENCY` | | 延迟测试并发数 | `200` |
| `CF_LATENCY_TOP` | | 延迟最低取多少个进行测速 | `10` |
| `CF_SPEED_DURATION` | | 每个 IP 测速秒数 | `5` |
| `CF_LOG_FILE` | | 日志文件路径 | `/tmp/cfip.log` |

---

## 运行方式

### 二进制直接运行

```bash
# 下载（amd64）
curl -L https://github.com/yourname/cfiptest/releases/latest/download/cfiptest-linux-amd64 \
  -o cfiptest && chmod +x cfiptest

# 设置环境变量并运行
export CF_API_TOKEN=your_token
export CF_ZONE_ID=your_zone_id
export CF_DOMAIN_1=cf1.yourdomain.com
export CF_DOMAIN_2=cf2.yourdomain.com
export CF_DOMAIN_3=cf3.yourdomain.com
export CF_CRON="0 3 * * *"

./cfiptest
```

### Docker

```bash
docker run -d \
  --name cfiptest \
  --restart unless-stopped \
  -e CF_API_TOKEN=your_token \
  -e CF_ZONE_ID=your_zone_id \
  -e CF_DOMAIN_1=cf1.yourdomain.com \
  -e CF_DOMAIN_2=cf2.yourdomain.com \
  -e CF_DOMAIN_3=cf3.yourdomain.com \
  -e CF_CRON="0 3 * * *" \
  -v /tmp/cfip.log:/tmp/cfip.log \
  yourname/cfiptest:latest
```

### Docker Compose

```yaml
version: "3.8"
services:
  cfiptest:
    image: yourname/cfiptest:latest
    restart: unless-stopped
    environment:
      CF_API_TOKEN: your_token
      CF_ZONE_ID: your_zone_id
      CF_DOMAIN_1: cf1.yourdomain.com
      CF_DOMAIN_2: cf2.yourdomain.com
      CF_DOMAIN_3: cf3.yourdomain.com
      CF_CRON: "0 3 * * *"
      CF_LATENCY_TOP: "10"
      CF_SPEED_DURATION: "5"
    volumes:
      - /tmp/cfip.log:/tmp/cfip.log
```

### systemd 服务（二进制方式）

```ini
# /etc/systemd/system/cfiptest.service
[Unit]
Description=Cloudflare IP Test & DNS Sync
After=network-online.target

[Service]
Type=simple
Restart=always
RestartSec=5
EnvironmentFile=/etc/cfiptest.env
ExecStart=/usr/local/bin/cfiptest

[Install]
WantedBy=multi-user.target
```

```bash
# /etc/cfiptest.env
CF_API_TOKEN=your_token
CF_ZONE_ID=your_zone_id
CF_DOMAIN_1=cf1.yourdomain.com
CF_DOMAIN_2=cf2.yourdomain.com
CF_DOMAIN_3=cf3.yourdomain.com
CF_CRON=0 3 * * *
```

```bash
systemctl enable --now cfiptest
```

---

## 日志示例

```
[2024-01-15 03:00:00] cfiptest 启动
[2024-01-15 03:00:00] 定时规则: 0 3 * * *
[2024-01-15 03:00:00] 域名: cf1.yourdomain.com | cf2.yourdomain.com | cf3.yourdomain.com
[2024-01-15 03:00:00] ========================================
[2024-01-15 03:00:00] 任务开始
[2024-01-15 03:00:01] 正在拉取优选 IP 列表: https://...
[2024-01-15 03:00:02] 获取到 500 个候选 IP
[2024-01-15 03:00:02] 开始并发延迟测试（并发数 200，端口 443）...
[2024-01-15 03:00:18] 延迟测试完成，可用 423 个，取延迟最低 10 个进行测速:
[2024-01-15 03:00:18]   #1   104.21.3.x        12ms
[2024-01-15 03:00:18]   #2   172.67.x.x        15ms
[2024-01-15 03:00:18]   #3   162.159.x.x       18ms
[2024-01-15 03:00:18]   ...
[2024-01-15 03:00:18] 开始测速（每个 IP 测速 5 秒）...
[2024-01-15 03:01:08] 测速完成，结果排名:
[2024-01-15 03:01:08]   #1   104.21.3.x        延迟 12ms  速度 8.24 MB/s
[2024-01-15 03:01:08]   #2   172.67.x.x        延迟 15ms  速度 6.51 MB/s
[2024-01-15 03:01:08]   #3   104.16.x.x        延迟 22ms  速度 5.83 MB/s
[2024-01-15 03:01:08] ----------------------------------------
[2024-01-15 03:01:08] 最优 Top3 IP:
[2024-01-15 03:01:08]   #1   104.21.3.x        延迟 12ms  速度 8.24 MB/s
[2024-01-15 03:01:08]   #2   172.67.x.x        延迟 15ms  速度 6.51 MB/s
[2024-01-15 03:01:08]   #3   104.16.x.x        延迟 22ms  速度 5.83 MB/s
[2024-01-15 03:01:08] ----------------------------------------
[2024-01-15 03:01:08] 开始同步 DNS 记录...
[2024-01-15 03:01:09] DNS 更新成功: cf1.yourdomain.com → 104.21.3.x
[2024-01-15 03:01:09] DNS 更新成功: cf2.yourdomain.com → 172.67.x.x
[2024-01-15 03:01:09] DNS 更新成功: cf3.yourdomain.com → 104.16.x.x
[2024-01-15 03:01:09] 任务完成，准备重启进程以释放内存...
[2024-01-15 03:01:09] ========================================
```

---

## GitHub Actions Secrets 配置

在仓库 Settings → Secrets 中添加：

| Secret | 说明 |
|--------|------|
| `DOCKERHUB_USERNAME` | Docker Hub 用户名 |
| `DOCKERHUB_TOKEN` | Docker Hub Access Token |

触发发布：推送 tag 即可

```bash
git tag v1.0.0
git push origin v1.0.0
```
