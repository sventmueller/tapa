Tapa 1.0.3 mở không gian làm việc trên máy tính khác qua SSH và gợi ý lệnh khi bạn gõ.

## Mới
- Không gian làm việc từ xa: "Không gian làm việc mới" cho phép chọn một thư mục trên máy tính khác: một máy chủ từ `~/.ssh/config` hoặc một địa chỉ, cổng, người dùng và mật khẩu khi cần, và thư mục ở đó. Các terminal và các tác nhân được cài ở đó chạy trên máy tính ấy qua một kết nối SSH duy nhất. Mật khẩu có thể được lưu trong chuỗi khóa, và khóa máy chủ lạ không bao giờ được chấp nhận khi chưa xác nhận. Danh sách tệp, bảng Git, trình soạn thảo và bản xem trước không có ở đó.
- Thanh bên hiển thị kết nối của không gian làm việc từ xa cùng "Kết nối lại", "Ngắt kết nối" và "Sửa kết nối". Khi kết nối bị mất quay lại, các terminal khởi động lại và các tab tác nhân tiếp tục cuộc trò chuyện.
- Gợi ý khi gõ: từ ký tự thứ hai, phần còn lại của lệnh được gợi ý hiện mờ sau con trỏ, và → nhận nó. Tab nhận một gợi ý chắc chắn hoặc mở menu hoàn thành mô tả từng ứng viên, có hoặc không có trường nhập.
- Màu lệnh: lệnh, lệnh con, tùy chọn, biến và toán tử mang màu của chủ đề, và lệnh mà shell không chạy được có gạch chân đứt nét màu đỏ. "Cài đặt › Nhập liệu" tắt gợi ý và màu.
- Trong tab tác nhân, các nút cho tác nhân con và tiến trình nền cùng /tasks mở chế độ xem hoạt động với trạng thái và thời gian chạy của từng mục, kèm "Hiển thị" và "Dừng".

## Cải tiến
- "Cài đặt › Chung › Cửa sổ" có thể tắt việc đổi tên không gian làm việc bằng cách bấm vào tên.
- Thanh bên hiển thị số tệp đã thay đổi, nút Git và nút thư mục theo thứ tự này, và chỉ hiện nút Git bên trong kho Git.
