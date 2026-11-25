Tapa 1.0.0 adalah rilis pertama berupa image disk: buka `Tapa-1.0.0.dmg`, lalu seret Tapa ke folder Aplikasi. Tapa 0.1.0 memperbarui dirinya ke versi ini, dan Homebrew memasang image yang sama.

## Baru
- Menutup jendela terakhir Tapa menutup Tapa dan menyimpan semuanya: ruang kerja, tab, pisahan, riwayat, dan sesi agen kembali saat Tapa dimulai berikutnya seperti yang Anda tinggalkan. Tapa bertanya lebih dulu hanya jika masih ada program selain shell yang berjalan.
- Seret ruang kerja di bilah samping ke urutan mana pun; "Terminal" selalu tetap di paling atas.
- Jendela baru terbuka dengan ukuran dan lebar bilah samping dari jendela yang terakhir Anda ubah ukurannya, juga saat Tapa tidak memulihkan jendela apa pun, dan Tapa mengingat apakah bidang input ditampilkan.
- Saat Tapa dimulai dengan bilah samping diciutkan, tombol "Ruang Kerja Baru" tetap tersembunyi.
- Output yang ditulis program sekaligus dalam satu semburan ditampilkan begitu program berhenti sejenak, paling lambat 8 ms kemudian, sehingga layar yang baru setengah tergambar tidak lagi berkedip.
- Setiap sesi berjalan di core terminal milik Tapa sendiri; pengaturan yang beralih ke core Alacritty sudah dihapus.
- Folder dan branch di atas bidang input berupa teks abu-abu polos, sejajar dengan placeholder.
- Di tab agen, `/resume` mencantumkan sesi lama agen di folder ini dan melanjutkan salah satunya di tab tersendiri.
- Daftar yang panjang di overlay tab agen bisa digulir dan tidak membuat jendela menjadi lebih tinggi.
- Agen dianggap selesai hanya setelah tiga detik tidak aktif, sehingga statusnya tidak lagi berkedip di antara langkah.
- Dalam layar penuh, bilah samping tetap berada di bawah bar alat dan ikut bergeser turun bersamanya saat bar menu muncul.

## Sekilas tentang Tapa
- Ruang kerja di bilah samping, masing-masing dengan tab, pisahan, agen, editor, dan tab browser-nya; jendela dan sesi agen kembali setelah mulai ulang.
- Tab agen untuk Claude, Codex, OpenCode, dan agen ACP mana pun dari registry, dipasang, diperbarui, dan dikembalikan ke versi sebelumnya dari pengaturan.
- Panel File dan Git, pratinjau Markdown dan gambar, bidang input perintah dengan dikte.
- Tampilan terang, gelap, atau mengikuti sistem, empat ukuran font, 35 bahasa antarmuka.
