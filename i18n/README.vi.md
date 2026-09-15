<div align="center">

# ⚡ NullRouter

**Bộ định tuyến AI & Tối ưu hóa Token không sao chép (Zero-Copy) hiệu năng cực cao cho Lập trình Agentic**

[![Rust](https://img.shields.io/badge/rust-phi%C3%AAn%20b%E1%BA%A3n%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/%C4%91%C6%B0%E1%BB%A3c%20cung%20c%E1%BA%A5p%20b%E1%BB%9Fi-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: EPL-2.0](https://img.shields.io/badge/License-EPL--2.0-blue.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/%C4%91%E1%BB%99%20tr%E1%BB%85%20%C4%91i%E1%BB%81u%20ph%E1%BB%91i-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/Giao%20di%E1%BB%87n-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/nh%C3%A0%20cung%20c%E1%BA%A5p-40%2B%20%C4%91%C3%A3%20t%C3%ADch%20h%E1%BB%A3p-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>Không bao giờ gián đoạn quá trình lập trình. Tiết kiệm 20–40% token với RTK, điều phối dưới một mili-giây và tự động chuyển đổi dự phòng thông minh qua hơn 40 nhà cung cấp AI.</b>
</p>

[🚀 Bắt đầu nhanh](#-b%E1%BA%AFt-%C4%91%E1%BA%A7u-nhanh) • [✨ Tính năng chính](#-t%C3%ADnh-n%C4%83ng-ch%C3%ADnh) • [🔌 Tích hợp](#-t%C3%ADch-h%E1%BB%A3p-c%C3%B4ng-c%E1%BB%A5) • [🌐 Nhà cung cấp](#-nh%C3%A0-cung-c%E1%BA%A5p-h%E1%BB%97-tr%E1%BB%A3) • [📊 Điểm chuẩn](#-b%E1%BA%A3ng-so-s%C3%A1nh) • [📖 Wiki](../wiki/Home.md) • [🩺 Chẩn đoán](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 Đọc bằng các ngôn ngữ khác

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 Tại sao chọn NullRouter?

Các công cụ lập trình tự hành (Claude Code, Cursor, Cline, Roo Code, Codex) liên tục gửi lượng lớn ngữ cảnh và kết quả công cụ. Các proxy cũ viết bằng Node.js hoặc Python thường gặp hiện tượng dừng thu gom rác (GC pauses), tiêu hao RAM lớn và độ trễ cao.

**NullRouter được xây dựng hoàn toàn bằng Rust trên nền tảng Cloudflare Pingora:**

| Tính năng | NullRouter (Rust + Pingora) | Bộ định tuyến Node.js truyền thống |
| :--- | :--- | :--- |
| **Engine Proxy** | Cloudflare Pingora (HTTP bất đồng bộ zero-copy) | Express / Koa / Fastify |
| **Độ trễ định tuyến** | **< 50 micro-giây** (đường truyền nhanh) | 15 – 45 mili-giây |
| **Bộ nhớ RAM chiếm dụng** | **~18 MB** khi nhàn rỗi | 120 – 350 MB |
| **Cấp phát luồng SSE** | Ghi trực tiếp vào bộ đệm đầu ra (`0` lần cấp phát trung gian) | Nối chuỗi liên tục và giải mã JSON lặp lại |
| **Bộ nén RTK** | Nén thông minh kết quả công cụ (tiết kiệm 20–40%) | Không nén |
| **Chuẩn hóa chuỗi suy nghĩ**| Hỗ trợ đầy đủ Claude 4, DeepSeek R1/V3.2, o3/o4 | Dễ bị mất thẻ `<think>` |
| **Bảng điều khiển** | Leptos WebAssembly SPA hỗ trợ 35 ngôn ngữ | Các gói Webpack/React nặng nề |

---

## ⚡ Bắt đầu nhanh

### 1. Khởi chạy NullRouter

```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

Hoặc sử dụng script khởi động:
```bash
./run.sh
```

Cổng truy cập:
- **API Endpoint**: `http://localhost:20128/v1`
- **Bảng điều khiển**: `http://localhost:20128/dashboard`
- **Sân chơi Chat trực tiếp**: `http://localhost:20128/dashboard/chat`

---

### 2. Cấu hình công cụ của bạn

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. Vào **Cursor Settings** ➔ **Models**.
2. Kích hoạt **OpenAI API Key** và điền `nullrouter-local`.
3. Ghi đè **OpenAI Base URL**: `http://localhost:20128/v1`.
4. Thêm mô hình muốn sử dụng (`claude-sonnet-4-6`, `deepseek-v4-pro`, `auto`).

---

## 📖 Toàn bộ tài liệu trên GitHub Wiki

Truy cập [NullRouter GitHub Wiki](../wiki/Home.md) để xem chi tiết:
- 📘 [Hướng dẫn cài đặt & khởi chạy](../wiki/Getting-Started.md)
- 🏗️ [Cấu trúc Pingora và cơ chế Zero-Copy](../wiki/Architecture-and-Performance.md)
- 💻 [Hướng dẫn kết nối các IDE](../wiki/Client-Integrations.md)
- 🔑 [Cấu hình 40+ nhà cung cấp & xoay vòng tài khoản](../wiki/Provider-Configuration.md)
- 🩺 [Cẩm nang chẩn đoán lỗi & xử lý sự cố toàn diện](../wiki/Diagnostics-and-Troubleshooting.md)
- 🇻🇳 [Hướng dẫn chẩn đoán Tiếng Việt](../wiki/i18n-Diagnostics-Guide-vi.md)

---

## 📄 Giấy phép

NullRouter được phát hành dưới giấy phép mã nguồn mở [EPL-2.0](LICENSE).
