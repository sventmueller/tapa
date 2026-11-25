Tapa 1.0.3 membuka ruang kerja pada komputer lain melalui SSH dan mencadangkan arahan semasa anda menaip.

## Baharu
- Ruang kerja jauh: "Ruang Kerja Baharu" menawarkan folder pada komputer lain: hos daripada `~/.ssh/config` atau alamat, port, pengguna dan kata laluan jika perlu, serta folder di sana. Terminal dan ejen yang dipasang di sana berjalan pada komputer itu melalui satu sambungan SSH. Kata laluan boleh disimpan dalam rantai kunci, dan kunci hos yang tidak dikenali tidak pernah diterima tanpa pengesahan. Senarai fail, panel Git, editor dan pratonton tidak tersedia di sana.
- Bar sisi menunjukkan sambungan ruang kerja jauh dengan "Sambung Semula", "Putuskan Sambungan" dan "Edit Sambungan". Apabila sambungan yang terputus kembali, terminal bermula semula dan tab ejen meneruskan perbualannya.
- Cadangan semasa menaip: mulai aksara kedua, baki arahan yang dicadangkan muncul pudar selepas kursor, dan → menerimanya. Tab menerima cadangan yang pasti atau membuka menu pelengkapan yang menerangkan setiap calon, dengan atau tanpa medan input.
- Warna arahan: arahan, subarahan, pilihan, pemboleh ubah dan operator mengambil warna tema, dan arahan yang tidak dapat dijalankan oleh shell mendapat garis bawah merah putus-putus. "Seting › Input" mematikan cadangan dan warna.
- Dalam tab ejen, butang untuk subejen dan proses latar belakang serta /tasks membuka paparan aktiviti dengan status dan masa berjalan setiap item, beserta "Tunjukkan" dan "Henti".

## Penambahbaikan
- "Seting › Umum › Tetingkap" boleh mematikan penamaan semula ruang kerja dengan mengklik namanya.
- Bar sisi menunjukkan bilangan fail yang berubah, butang Git dan butang folder mengikut susunan ini, dan butang Git hanya di dalam repositori Git.
