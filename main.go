package main

import (
	"bufio"
	"bytes"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"math/rand"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

// ── Config ────────────────────────────────────────────────────────────────────

type Config struct {
	APIToken    string
	ZoneID      string
	Domains     [3]string
	CronExpr    string
	IPListURL   string
	Concurrency int
	LatencyTop  int
	SpeedSecs   int
	LogFile     string
	Port        int
}

func loadConfig() Config {
	c := Config{
		APIToken:    mustEnv("CF_API_TOKEN"),
		ZoneID:      mustEnv("CF_ZONE_ID"),
		CronExpr:    getEnv("CF_CRON", "0 3 * * *"),
		IPListURL:   getEnv("CF_IP_LIST_URL", "https://raw.githubusercontent.com/ymyuuu/Cloudflare-Better-IP/main/bestip.txt"),
		Concurrency: getEnvInt("CF_TEST_CONCURRENCY", 200),
		LatencyTop:  getEnvInt("CF_LATENCY_TOP", 10),
		SpeedSecs:   getEnvInt("CF_SPEED_DURATION", 5),
		LogFile:     getEnv("CF_LOG_FILE", "/tmp/cfip.log"),
		Port:        443,
	}
	c.Domains[0] = mustEnv("CF_DOMAIN_1")
	c.Domains[1] = mustEnv("CF_DOMAIN_2")
	c.Domains[2] = mustEnv("CF_DOMAIN_3")
	return c
}

func mustEnv(k string) string {
	v := os.Getenv(k)
	if v == "" {
		fmt.Fprintf(os.Stderr, "ERROR: env %s is required\n", k)
		os.Exit(1)
	}
	return v
}

func getEnv(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

func getEnvInt(k string, def int) int {
	if v := os.Getenv(k); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
	}
	return def
}

// ── Logger ────────────────────────────────────────────────────────────────────

var logger *log.Logger

func initLogger(path string) (*os.File, error) {
	f, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0644)
	if err != nil {
		return nil, err
	}
	mw := io.MultiWriter(os.Stdout, f)
	logger = log.New(mw, "", 0)
	return f, nil
}

func logf(format string, args ...any) {
	ts := time.Now().Format("2006-01-02 15:04:05")
	logger.Printf("[%s] %s", ts, fmt.Sprintf(format, args...))
}

// ── IP list ───────────────────────────────────────────────────────────────────

func fetchIPs(url string) ([]string, error) {
	resp, err := http.Get(url)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	var (
		ips     []string
		lines   int
		skipped int
		cidrs   int
		plain   int
	)

	sc := bufio.NewScanner(resp.Body)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		lines++

		// Strip port if present: "1.2.3.4:443"
		host := line
		if h, _, err2 := net.SplitHostPort(line); err2 == nil {
			host = h
		}

		// Plain IP
		if net.ParseIP(host) != nil {
			ips = append(ips, host)
			plain++
			continue
		}

		// CIDR: expand and sample up to 256 IPs per block
		if strings.Contains(host, "/") {
			expanded := expandCIDR(host)
			if len(expanded) > 0 {
				ips = append(ips, expanded...)
				cidrs++
				continue
			}
		}

		skipped++
	}

	logf("IP 列表解析: 原始行 %d 条，纯IP %d 个，CIDR块 %d 个，跳过 %d 条，合计候选 %d 个",
		lines, plain, cidrs, skipped, len(ips))

	return ips, sc.Err()
}

// expandCIDR returns up to 256 valid host IPs from a CIDR block using random sampling.
func expandCIDR(cidr string) []string {
	_, ipNet, err := net.ParseCIDR(cidr)
	if err != nil {
		return nil
	}

	ones, bits := ipNet.Mask.Size()
	if bits != 32 {
		return nil // IPv6 not supported
	}
	hostBits := bits - ones
	if hostBits == 0 {
		return nil // single host, no valid range
	}

	// Convert network base to uint32 for easy arithmetic
	base := ipNet.IP.To4()
	if base == nil {
		return nil
	}
	baseInt := uint32(base[0])<<24 | uint32(base[1])<<16 | uint32(base[2])<<8 | uint32(base[3])

	totalHosts := uint32(1) << hostBits
	// skip network address (+1) and broadcast (-1)
	first := baseInt + 1
	last := baseInt + totalHosts - 2
	if first > last {
		return nil
	}

	count := last - first + 1
	const maxPerCIDR = 200

	var ips []string
	if count <= maxPerCIDR {
		// Small block: take all
		for n := first; n <= last; n++ {
			ips = append(ips, uint32ToIP(n))
		}
	} else {
		// Large block: random sample without replacement
		seen := make(map[uint32]bool, maxPerCIDR)
		tries := 0
		for len(ips) < maxPerCIDR && tries < maxPerCIDR*10 {
			tries++
			offset := rand.Uint32()%count + first
			if seen[offset] {
				continue
			}
			seen[offset] = true
			ips = append(ips, uint32ToIP(offset))
		}
	}
	return ips
}

func uint32ToIP(n uint32) string {
	return fmt.Sprintf("%d.%d.%d.%d", n>>24, (n>>16)&0xff, (n>>8)&0xff, n&0xff)
}

func isNetOrBroadcast(ip net.IP, ipNet *net.IPNet) bool {
	ip4 := ip.To4()
	if ip4 == nil {
		return false
	}
	net4 := ipNet.IP.To4()
	mask := ipNet.Mask
	// network address
	isNet := true
	for i := range ip4 {
		if ip4[i] != net4[i] {
			isNet = false
			break
		}
	}
	if isNet {
		return true
	}
	// broadcast
	for i := range ip4 {
		if ip4[i] != (net4[i] | ^mask[i]) {
			return false
		}
	}
	return true
}

// ── Latency test ──────────────────────────────────────────────────────────────

type latResult struct {
	IP      string
	Latency time.Duration
}

func testLatency(ip string, port int, timeout time.Duration) (time.Duration, error) {
	addr := net.JoinHostPort(ip, strconv.Itoa(port))
	start := time.Now()
	conn, err := net.DialTimeout("tcp", addr, timeout)
	if err != nil {
		return 0, err
	}
	conn.Close()
	return time.Since(start), nil
}

func runLatencyTests(ips []string, port, concurrency int) []latResult {
	sem := make(chan struct{}, concurrency)
	var mu sync.Mutex
	var results []latResult
	var wg sync.WaitGroup

	for _, ip := range ips {
		wg.Add(1)
		sem <- struct{}{}
		go func(ip string) {
			defer wg.Done()
			defer func() { <-sem }()
			lat, err := testLatency(ip, port, 2*time.Second)
			if err != nil {
				return
			}
			mu.Lock()
			results = append(results, latResult{IP: ip, Latency: lat})
			mu.Unlock()
		}(ip)
	}
	wg.Wait()

	sort.Slice(results, func(i, j int) bool {
		return results[i].Latency < results[j].Latency
	})
	return results
}

// ── Speed test ────────────────────────────────────────────────────────────────

type speedResult struct {
	IP      string
	Latency time.Duration
	Speed   float64 // MB/s
}

func testSpeed(ip string, port int, durationSecs int) (float64, error) {
	const testHost = "speed.cloudflare.com"
	addr := net.JoinHostPort(ip, strconv.Itoa(port))

	deadline := time.Now().Add(time.Duration(durationSecs) * time.Second)
	var totalBytes int64
	var lastErr error

	// Keep opening connections and downloading until deadline
	for time.Now().Before(deadline) {
		conn, err := net.DialTimeout("tcp", addr, 3*time.Second)
		if err != nil {
			lastErr = fmt.Errorf("TCP连接失败: %v", err)
			break
		}

		tlsCfg := &tls.Config{
			InsecureSkipVerify: false,
			ServerName:         testHost,
			NextProtos:         []string{"http/1.1"}, // force HTTP/1.1, disable h2
		}
		tlsConn := tls.Client(conn, tlsCfg)
		remaining := time.Until(deadline)
		if remaining <= 0 {
			conn.Close()
			break
		}
		tlsConn.SetDeadline(time.Now().Add(remaining))

		if err := tlsConn.Handshake(); err != nil {
			tlsConn.Close()
			lastErr = fmt.Errorf("TLS握手失败: %v", err)
			break
		}

		reqStr := "GET /__down?bytes=104857600 HTTP/1.1\r\nHost: " + testHost + "\r\nUser-Agent: Mozilla/5.0\r\nAccept: */*\r\nConnection: close\r\n\r\n"
		if _, err := tlsConn.Write([]byte(reqStr)); err != nil {
			tlsConn.Close()
			lastErr = fmt.Errorf("发送请求失败: %v", err)
			break
		}

		buf := make([]byte, 32*1024)
		var headerBuf []byte
		headerDone := false
		isFirstConn := totalBytes == 0 && lastErr == nil
		var connBytes int64

		for time.Now().Before(deadline) {
			n, readErr := tlsConn.Read(buf)
			if n > 0 {
				if !headerDone {
					headerBuf = append(headerBuf, buf[:n]...)
					if idx := bytes.Index(headerBuf, []byte("\r\n\r\n")); idx >= 0 {
						headerDone = true
						bodyPart := int64(len(headerBuf) - idx - 4)
						totalBytes += bodyPart
						connBytes += bodyPart
						// Log first response header for diagnosis
						if isFirstConn {
							firstLine := string(headerBuf[:bytes.IndexByte(headerBuf, '\r')])
							logf("测速诊断 %s: 响应 [%s] header=%d字节 body起始=%d字节",
								ip, firstLine, idx, bodyPart)
						}
						headerBuf = nil
					}
				} else {
					totalBytes += int64(n)
					connBytes += int64(n)
				}
			}
			if readErr != nil {
				break
			}
		}
		if isFirstConn && !headerDone {
			logf("测速诊断 %s: 未找到响应头分隔符，收到 %d 字节原始数据: %q",
				ip, len(headerBuf), headerBuf[:min(len(headerBuf), 200)])
		}
		tlsConn.Close()
	}

	if totalBytes == 0 {
		if lastErr != nil {
			return 0, lastErr
		}
		return 0, fmt.Errorf("读取到 0 字节，请检查网络或IP可用性")
	}

	mbps := float64(totalBytes) / 1024 / 1024 / float64(durationSecs)
	return mbps, nil
}
func runSpeedTests(candidates []latResult, port, durationSecs int) []speedResult {
	var results []speedResult
	for _, c := range candidates {
		mbps, err := testSpeed(c.IP, port, durationSecs)
		if err != nil {
			logf("测速失败 %s: %v", c.IP, err)
			continue
		}
		results = append(results, speedResult{
			IP:      c.IP,
			Latency: c.Latency,
			Speed:   mbps,
		})
	}
	sort.Slice(results, func(i, j int) bool {
		return results[i].Speed > results[j].Speed
	})
	return results
}

// ── Cloudflare DNS API ────────────────────────────────────────────────────────

type cfRecord struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Type    string `json:"type"`
	Content string `json:"content"`
}

type cfListResp struct {
	Result []cfRecord `json:"result"`
	Success bool       `json:"success"`
}

type cfUpdateResp struct {
	Success bool `json:"success"`
	Errors  []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

func cfRequest(method, url, token string, body io.Reader) (*http.Response, error) {
	req, err := http.NewRequest(method, url, body)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")
	client := &http.Client{Timeout: 15 * time.Second}
	return client.Do(req)
}

func getRecordID(token, zoneID, name string) (string, error) {
	url := fmt.Sprintf("https://api.cloudflare.com/client/v4/zones/%s/dns_records?type=A&name=%s", zoneID, name)
	resp, err := cfRequest("GET", url, token, nil)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	var r cfListResp
	if err := json.NewDecoder(resp.Body).Decode(&r); err != nil {
		return "", err
	}
	if len(r.Result) > 0 {
		return r.Result[0].ID, nil
	}
	return "", nil
}

func upsertDNS(token, zoneID, name, ip string) error {
	recordID, err := getRecordID(token, zoneID, name)
	if err != nil {
		return err
	}

	payload := fmt.Sprintf(`{"type":"A","name":%q,"content":%q,"ttl":60,"proxied":false}`, name, ip)

	var (
		method string
		url    string
	)
	if recordID == "" {
		method = "POST"
		url = fmt.Sprintf("https://api.cloudflare.com/client/v4/zones/%s/dns_records", zoneID)
	} else {
		method = "PUT"
		url = fmt.Sprintf("https://api.cloudflare.com/client/v4/zones/%s/dns_records/%s", zoneID, recordID)
	}

	resp, err := cfRequest(method, url, token, strings.NewReader(payload))
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	var r cfUpdateResp
	if err := json.NewDecoder(resp.Body).Decode(&r); err != nil {
		return err
	}
	if !r.Success {
		if len(r.Errors) > 0 {
			return fmt.Errorf("%s", r.Errors[0].Message)
		}
		return fmt.Errorf("unknown error")
	}
	return nil
}

// ── Core job ──────────────────────────────────────────────────────────────────

func run(cfg Config) {
	logf("========================================")
	logf("任务开始")

	// 1. Fetch IP list
	logf("正在拉取优选 IP 列表: %s", cfg.IPListURL)
	ips, err := fetchIPs(cfg.IPListURL)
	if err != nil {
		logf("ERROR 拉取 IP 列表失败: %v", err)
		return
	}
	logf("获取到 %d 个候选 IP", len(ips))

	// 2. Latency test
	logf("开始并发延迟测试（并发数 %d，端口 %d）...", cfg.Concurrency, cfg.Port)
	latResults := runLatencyTests(ips, cfg.Port, cfg.Concurrency)
	if len(latResults) == 0 {
		logf("ERROR 延迟测试无可用 IP")
		return
	}

	top := cfg.LatencyTop
	if top > len(latResults) {
		top = len(latResults)
	}
	topLat := latResults[:top]

	logf("延迟测试完成，可用 %d 个，取延迟最低 %d 个进行测速:", len(latResults), top)
	for i, r := range topLat {
		logf("  #%-2d  %-16s  %v", i+1, r.IP, r.Latency.Round(time.Millisecond))
	}

	// 3. Speed test
	logf("开始测速（每个 IP 测速 %d 秒）...", cfg.SpeedSecs)
	speedResults := runSpeedTests(topLat, cfg.Port, cfg.SpeedSecs)
	if len(speedResults) == 0 {
		logf("ERROR 测速无可用结果")
		return
	}

	logf("测速完成，结果排名:")
	for i, r := range speedResults {
		logf("  #%-2d  %-16s  延迟 %v  速度 %.2f MB/s",
			i+1, r.IP, r.Latency.Round(time.Millisecond), r.Speed)
	}

	// 4. Pick top 3
	need := 3
	if len(speedResults) < need {
		need = len(speedResults)
	}
	top3 := speedResults[:need]

	logf("----------------------------------------")
	logf("最优 Top%d IP:", need)
	for i, r := range top3 {
		logf("  #%d  %-16s  延迟 %v  速度 %.2f MB/s",
			i+1, r.IP, r.Latency.Round(time.Millisecond), r.Speed)
	}

	// 5. Update DNS
	logf("----------------------------------------")
	logf("开始同步 DNS 记录...")
	for i, r := range top3 {
		domain := cfg.Domains[i]
		if err := upsertDNS(cfg.APIToken, cfg.ZoneID, domain, r.IP); err != nil {
			logf("DNS 更新失败: %s → %s : %v", domain, r.IP, err)
		} else {
			logf("DNS 更新成功: %s → %s", domain, r.IP)
		}
	}

	logf("任务完成，准备重启进程以释放内存...")
	logf("========================================")

	// 6. Self-restart to free memory
	restart()
}

func restart() {
	exe, err := os.Executable()
	if err != nil {
		logf("ERROR 获取可执行路径失败: %v", err)
		return
	}
	exe, _ = filepath.EvalSymlinks(exe)
	cmd := exec.Command(exe, os.Args[1:]...)
	cmd.Env = os.Environ()
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		logf("ERROR 重启失败: %v", err)
		return
	}
	os.Exit(0)
}

// ── Built-in cron (stdlib only) ───────────────────────────────────────────────

// parseCron parses a 5-field cron expression: minute hour dom month dow
// Returns a function that reports whether a given time matches.
func parseCron(expr string) (func(time.Time) bool, error) {
	fields := strings.Fields(expr)
	if len(fields) != 5 {
		return nil, fmt.Errorf("expected 5 fields, got %d", len(fields))
	}
	type matcher func(int) bool
	parse := func(field string, min, max int) (matcher, error) {
		if field == "*" {
			return func(int) bool { return true }, nil
		}
		// list: 1,2,3
		if strings.Contains(field, ",") {
			parts := strings.Split(field, ",")
			vals := make([]int, 0, len(parts))
			for _, p := range parts {
				n, err := strconv.Atoi(strings.TrimSpace(p))
				if err != nil || n < min || n > max {
					return nil, fmt.Errorf("invalid value %q", p)
				}
				vals = append(vals, n)
			}
			return func(v int) bool {
				for _, n := range vals {
					if v == n {
						return true
					}
				}
				return false
			}, nil
		}
		// step: */5 or 0-30/5
		if strings.Contains(field, "/") {
			parts := strings.SplitN(field, "/", 2)
			step, err := strconv.Atoi(parts[1])
			if err != nil || step <= 0 {
				return nil, fmt.Errorf("invalid step %q", parts[1])
			}
			start := min
			end := max
			if parts[0] != "*" {
				r := strings.SplitN(parts[0], "-", 2)
				start, err = strconv.Atoi(r[0])
				if err != nil {
					return nil, fmt.Errorf("invalid range start %q", r[0])
				}
				if len(r) == 2 {
					end, err = strconv.Atoi(r[1])
					if err != nil {
						return nil, fmt.Errorf("invalid range end %q", r[1])
					}
				}
			}
			return func(v int) bool {
				if v < start || v > end {
					return false
				}
				return (v-start)%step == 0
			}, nil
		}
		// range: 1-5
		if strings.Contains(field, "-") {
			parts := strings.SplitN(field, "-", 2)
			lo, err1 := strconv.Atoi(parts[0])
			hi, err2 := strconv.Atoi(parts[1])
			if err1 != nil || err2 != nil || lo > hi {
				return nil, fmt.Errorf("invalid range %q", field)
			}
			return func(v int) bool { return v >= lo && v <= hi }, nil
		}
		// exact
		n, err := strconv.Atoi(field)
		if err != nil || n < min || n > max {
			return nil, fmt.Errorf("invalid value %q", field)
		}
		return func(v int) bool { return v == n }, nil
	}

	mMin, err := parse(fields[0], 0, 59)
	if err != nil {
		return nil, fmt.Errorf("minute: %w", err)
	}
	mHour, err := parse(fields[1], 0, 23)
	if err != nil {
		return nil, fmt.Errorf("hour: %w", err)
	}
	mDom, err := parse(fields[2], 1, 31)
	if err != nil {
		return nil, fmt.Errorf("dom: %w", err)
	}
	mMon, err := parse(fields[3], 1, 12)
	if err != nil {
		return nil, fmt.Errorf("month: %w", err)
	}
	mDow, err := parse(fields[4], 0, 7)
	if err != nil {
		return nil, fmt.Errorf("dow: %w", err)
	}

	return func(t time.Time) bool {
		dow := int(t.Weekday()) // 0=Sunday
		return mMin(t.Minute()) &&
			mHour(t.Hour()) &&
			mDom(t.Day()) &&
			mMon(int(t.Month())) &&
			(mDow(dow) || mDow(dow+7)) // 7 == Sunday alias
	}, nil
}

// scheduleCron blocks forever, calling fn whenever the cron fires.
func scheduleCron(expr string, fn func()) error {
	matches, err := parseCron(expr)
	if err != nil {
		return err
	}
	go func() {
		lastFired := -1 // track last minute we fired, -1 = never fired
		for {
			now := time.Now()
			if matches(now) {
				min := now.Hour()*60 + now.Minute()
				if min != lastFired {
					lastFired = min
					go fn()
				}
			}
			// Sleep until the next minute tick
			next := now.Truncate(time.Minute).Add(time.Minute)
			time.Sleep(time.Until(next))
		}
	}()
	select {} // block main
}

// ── Entry point ───────────────────────────────────────────────────────────────

func main() {
	cfg := loadConfig()

	f, err := initLogger(cfg.LogFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "ERROR: cannot open log file %s: %v\n", cfg.LogFile, err)
		os.Exit(1)
	}
	defer f.Close()

	logf("cfiptest 启动")
	logf("日志文件: %s", cfg.LogFile)
	logf("定时规则: %s", cfg.CronExpr)
	logf("域名: %s | %s | %s", cfg.Domains[0], cfg.Domains[1], cfg.Domains[2])
	logf("IP 列表: %s", cfg.IPListURL)
	logf("并发数: %d  延迟Top: %d  测速秒数: %d", cfg.Concurrency, cfg.LatencyTop, cfg.SpeedSecs)

	// Schedule via built-in cron (no immediate run on start)
	if err := scheduleCron(cfg.CronExpr, func() { run(cfg) }); err != nil {
		logf("ERROR cron 表达式解析失败: %v", err)
		os.Exit(1)
	}
}
