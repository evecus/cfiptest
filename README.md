# P-BOX Rust Cloudflare 测速工具（CLI）

这个版本**不监听端口**，直接运行二进制文件 + 参数即可测速，并在结束后输出 JSON 结果。

## 1) 构建

```bash
cd backend-rust
cargo build --release
```

生成文件：

- `target/release/pbox-speedtest-backend`

## 2) 直接运行

```bash
./target/release/pbox-speedtest-backend
```

## 3) 参数说明

```bash
./pbox-speedtest-backend \
  --threads 4 \
  --upload-threads 2 \
  --ping-count 3 \
  --download-seconds 15 \
  --upload-seconds 10
```

- `--threads`：下载并发线程数（默认 `4`）
- `--upload-threads`：上传并发线程数（默认 `2`）
- `--ping-count`：TCP ping 次数（默认 `3`）
- `--download-seconds`：下载测速持续秒数（默认 `15`）
- `--upload-seconds`：上传测速持续秒数（默认 `10`）

帮助命令：

```bash
./pbox-speedtest-backend --help
```

## 4) 输出示例

程序结束后输出 JSON（标准输出）：

```json
{
  "id": 1777777777777,
  "ping": 11.9,
  "download_speed": 325.74,
  "upload_speed": 43.62,
  "source": "cloudflare",
  "threads": 4,
  "upload_threads": 2,
  "timestamp": "2026-05-15T12:34:56.000000+00:00"
}
```

## 5) GitHub 自动构建 Linux amd64/arm64

工作流文件：`.github/workflows/rust-backend.yml`

每次变更 `backend-rust/**` 时，GitHub Actions 会：

1. 执行格式与测试检查
2. 编译以下目标平台二进制文件并上传 artifacts：
   - `x86_64-unknown-linux-gnu`（Linux amd64）
   - `aarch64-unknown-linux-gnu`（Linux arm64）
