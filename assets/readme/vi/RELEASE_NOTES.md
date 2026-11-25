Tapa 1.0.0 là bản phát hành đầu tiên dưới dạng ảnh đĩa: mở `Tapa-1.0.0.dmg` và kéo Tapa vào thư mục Ứng dụng. Tapa 0.1.0 tự cập nhật lên bản này, và Homebrew cài cùng ảnh đĩa đó.

## Mới
- Đóng cửa sổ cuối cùng của Tapa sẽ thoát Tapa và giữ lại mọi thứ: không gian làm việc, tab, khung chia, lịch sử và phiên tác nhân trở lại ở lần khởi động tiếp theo đúng như bạn đã để. Tapa chỉ hỏi trước khi vẫn còn một chương trình khác shell đang chạy.
- Kéo các không gian làm việc trên thanh bên sang thứ tự bất kỳ; "Terminal" luôn nằm trên cùng.
- Cửa sổ mới mở ra với kích thước và độ rộng thanh bên của cửa sổ bạn đổi kích thước gần nhất, kể cả khi Tapa không khôi phục cửa sổ nào, và Tapa nhớ trường nhập có đang hiện hay không.
- Khi Tapa khởi động với thanh bên đang thu gọn, nút "Không gian làm việc mới" vẫn được ẩn.
- Kết quả đầu ra mà một chương trình ghi dồn dập sẽ hiện ra khi chương trình tạm dừng, chậm nhất sau 8 ms, nên các màn hình vẽ dở không còn nhấp nháy.
- Mọi phiên đều chạy trên lõi terminal riêng của Tapa; tùy chọn trong Cài đặt từng chuyển sang lõi của Alacritty đã bị loại bỏ.
- Thư mục và nhánh phía trên trường nhập là văn bản màu xám thuần, căn thẳng hàng với văn bản gợi ý.
- Trong tab tác nhân, `/resume` liệt kê các phiên trước đây của tác nhân trong thư mục này và tiếp tục một phiên trong tab riêng.
- Một danh sách dài trong lớp phủ của tab tác nhân sẽ cuộn thay vì làm cửa sổ cao thêm.
- Tác nhân chỉ được coi là đã xong sau ba giây nhàn rỗi, nên trạng thái của nó không còn nhấp nháy giữa các bước.
- Ở chế độ toàn màn hình, thanh bên nằm dưới thanh công cụ và trượt xuống cùng nó khi thanh menu hiện ra.

## Tổng quan về Tapa
- Không gian làm việc trên thanh bên, mỗi không gian có tab, khung chia, tác nhân, trình soạn thảo và tab trình duyệt riêng; cửa sổ và phiên tác nhân trở lại sau khi khởi động lại.
- Tab tác nhân cho Claude, Codex, OpenCode và mọi tác nhân ACP trong registry, được cài đặt, cập nhật và quay về phiên bản trước ngay trong Cài đặt.
- Bảng Tệp và Git, bản xem trước Markdown và hình ảnh, trường nhập lệnh có đọc chính tả.
- Chế độ sáng, tối hoặc theo hệ thống, bốn cỡ chữ, 35 ngôn ngữ giao diện.
