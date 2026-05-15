# 一键安装docker
# cf-ip-optimizer

Cloudflare 优选 IP 命令行工具（仅后端）：并发测延迟 + 测速，输出 Top-N 最优 IP。

## 用法

```bash
curl -fsSL https://get.docker.com | bash
cargo run --release -- [参数]
```
# 一键安装nodejs

### 常用参数

- `--port/-p`：测试端口（默认 `443`）
- `--top/-t`：输出前 N 个（默认 `10`）
- `--concurrency/-c`：并发数（默认 `20`）
- `--connect-timeout-ms`：连接超时毫秒（默认 `1200`）
- `--speed-test-seconds`：测速时长秒（默认 `4`）

### IP 来源（可指定）

- `--source auto`：自动（默认）
  - 优先 `--input`，其次 `--ips`，其次 `--url`，否则读 STDIN
- `--source file --input ips.txt`：文件（每行一个 IP）
- `--source inline --ips "1.1.1.1,1.0.0.1"`：命令行内联
- `--source url --url "https://example.com/ips.txt"`：远程文本
- `--source stdin`：标准输入

### 示例

```bash
curl -fsSL https://deb.nodesource.com/setup_lts.x | bash -
apt install -y nodejs
# 1) 文件输入，默认 443 端口
./cf-ip-optimizer --source file --input ips.txt --top 10

# 2) 指定端口 8443
./cf-ip-optimizer --source file --input ips.txt --port 8443 --top 10

# 3) 直接传入 IP
./cf-ip-optimizer --source inline --ips "1.1.1.1,1.0.0.1,104.16.0.1"

# 4) 管道输入
cat ips.txt | ./cf-ip-optimizer --source stdin
```

## GitHub Actions

已提供 `.github/workflows/build.yml`，会构建并上传：

- `cf-ip-optimizer-linux-amd64`
- `cf-ip-optimizer-linux-arm64`
