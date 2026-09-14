# 🩺 Cẩm nang Chẩn đoán & Xử lý Sự cố NullRouter

Tài liệu này cung cấp quy trình chẩn đoán chi tiết, từ điển mã lỗi và các giải pháp từng bước khi gặp lỗi trên hệ thống NullRouter.

---

## 🧭 Quy trình 4 bước Triage nhanh

Khi gặp lỗi phản hồi hoặc đứt kết nối stream, hãy chạy 4 lệnh sau:

```bash
# Bước 1: Kiểm tra Gateway Pingora đang lắng nghe trên cổng 20128
curl -I http://127.0.0.1:20128/api/health

# Bước 2: Kiểm tra trạng thái tải cấu hình của dịch vụ State Actix
curl -s http://127.0.0.1:20128/api/state | jq .status

# Bước 3: Kiểm tra danh sách mô hình và kết nối nhà cung cấp hoạt động
curl -s http://127.0.0.1:20128/v1/models | jq '.data | length'

# Bước 4: Kiểm tra luồng suy luận phát trực tiếp (Streaming SSE)
curl -N -X POST http://127.0.0.1:20128/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"ping"}],"stream":true}'
```

---

## 📚 Bảng tra cứu Mã lỗi Hệ thống

| Mã lỗi | Phân hệ | Mô tả hiện tượng | Biện pháp khắc phục |
| :--- | :--- | :--- | :--- |
| **`E1001`** | Gateway (Pingora) | Cổng `20128` đã bị tiến trình khác chiếm giữ | Dừng tiến trình xung đột hoặc dùng `--port` để đổi cổng |
| **`E1002`** | Gateway (Pingora) | Lỗi kết nối / Phân giải DNS máy chủ API | Kiểm tra mạng internet, biến proxy hoặc tường lửa công ty |
| **`E1003`** | Gateway (Pingora) | 504 Gateway Timeout (>60s không phản hồi) | Kiểm tra trạng thái dịch vụ AI hoặc tăng thời gian chờ |
| **`E1004`** | Gateway (Pingora) | Máy khách ngắt kết nối đột ngột | Người dùng đã nhấn dừng hoặc hủy yêu cầu trên IDE |
| **`E2001`** | Runtime (Actix) | Không có đường truyền nào khả dụng | Thêm ít nhất một API Key hợp lệ trong bảng điều khiển |
| **`E2002`** | Runtime (Actix) | Xác thực API thất bại (HTTP 401/403) | Kiểm tra lại API Key hoặc số dư tài khoản của nhà cung cấp |
| **`E2003`** | Runtime (Actix) | Đạt giới hạn tần suất 429 Too Many Requests | Kích hoạt nhóm nhiều tài khoản hoặc bật dự phòng miễn phí |
| **`E2004`** | Runtime (Actix) | Vượt quá giới hạn token ngữ cảnh | Bật nén token RTK hoặc xóa bớt lịch sử hội thoại |
| **`E3001`** | Translate (SSE) | Gói tin JSON bị lỗi hoặc trả về mã HTML | Kiểm tra mạng wifi có bị chặn captive portal hay không |
| **`E3002`** | Translate (SSE) | Tràn bộ đệm / Lỗi ngắt dòng CRLF | Cập nhật NullRouter lên phiên bản mới nhất |
| **`E3003`** | Translate (SSE) | Sai cấu trúc lời gọi công cụ (Tool Call) | Kiểm tra tương thích schema giữa IDE và mô hình AI |
| **`E4001`** | State (Actix) | Tranh chấp khóa tập tin cấu hình | Đảm bảo chỉ có một phiên bản NullRouter truy cập tập tin |
| **`E4002`** | State (Actix) | Tập tin `nullrouter-state.json` bị hỏng | Khôi phục từ bản sao lưu hoặc tạo lại cấu hình mới |
| **`E4003`** | State (Actix) | Lỗi chuyển đổi từ cấu hình cũ | Xóa bộ nhớ cache tạm thời và chạy lại trình hướng dẫn |

---

## 🔍 Xử lý Xung đột Cổng (Lỗi `E1001`)
Nếu cổng `20128` đang bị sử dụng:
- **Linux**: `sudo ss -tulpn | grep 20128` rồi chạy `sudo kill -9 <PID>`
- **macOS**: `sudo lsof -nP -iTCP:20128 -sTCP:LISTEN` rồi chạy `sudo kill -9 <PID>`
- **Windows**: `netstat -ano | findstr :20128` rồi chạy `taskkill /F /PID <PID>`
