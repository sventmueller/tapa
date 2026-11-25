Tapa 1.0.3 membuka ruang kerja di komputer lain melalui SSH dan menyarankan perintah saat Anda mengetik.

## Baru
- Ruang kerja jarak jauh: "Ruang Kerja Baru" menawarkan folder di komputer lain: host dari `~/.ssh/config` atau alamat, port, pengguna, dan kata sandi bila perlu, serta folder di sana. Terminal dan agen yang terpasang di sana berjalan di komputer itu melalui satu sambungan SSH. Kata sandi dapat disimpan di rantai kunci, dan kunci host yang tidak dikenal tidak pernah diterima tanpa konfirmasi. Daftar file, panel Git, editor, dan pratinjau tidak tersedia di sana.
- Bar samping menampilkan sambungan ruang kerja jarak jauh dengan "Sambungkan Ulang", "Putuskan Sambungan", dan "Edit Sambungan". Saat sambungan yang terputus kembali, terminal dimulai lagi dan tab agen melanjutkan percakapannya.
- Saran saat mengetik: mulai karakter kedua, sisa perintah yang disarankan tampak samar setelah kursor, dan → menerimanya. Tab menerima saran yang pasti atau membuka menu pelengkapan yang menjelaskan setiap kandidat, dengan atau tanpa kolom input.
- Warna perintah: perintah, subperintah, opsi, variabel, dan operator mendapat warna tema, dan perintah yang tidak dapat dijalankan shell mendapat garis bawah merah putus-putus. "Pengaturan › Input" mematikan saran dan warna.
- Di tab agen, tombol untuk subagen dan proses latar belakang serta /tasks membuka tampilan aktivitas dengan status dan waktu berjalan setiap item, beserta "Tampilkan" dan "Hentikan".

## Peningkatan
- "Pengaturan › Umum › Jendela" dapat mematikan penggantian nama ruang kerja dengan mengeklik namanya.
- Bar samping menampilkan jumlah file yang berubah, tombol Git, dan tombol folder dalam urutan ini, dan tombol Git hanya di dalam repositori Git.
