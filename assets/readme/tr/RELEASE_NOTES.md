Tapa 1.0.3, SSH üzerinden başka bilgisayarlarda çalışma alanları açar ve siz yazarken komut önerir.

## Yenilikler
- Uzak çalışma alanları: "Yeni Çalışma Alanı" başka bir bilgisayardaki bir klasörü de sunar: `~/.ssh/config` içinden bir sunucu veya bir adres, gerekirse bağlantı noktası, kullanıcı ve parola, ve oradaki klasör. Terminaller ve orada yüklü ajanlar tek bir SSH bağlantısı üzerinden o bilgisayarda çalışır. Parola anahtar zincirinde saklanabilir ve bilinmeyen bir sunucu anahtarı hiçbir zaman onay olmadan kabul edilmez. Dosya listesi, Git paneli, düzenleyici ve önizlemeler orada kullanılamaz.
- Kenar çubuğu, uzak bir çalışma alanının bağlantısını "Yeniden Bağlan", "Bağlantıyı Kes" ve "Bağlantıyı Düzenle" ile gösterir. Kopan bir bağlantı geri geldiğinde terminaller yeniden başlar ve ajan sekmeleri konuşmalarını sürdürür.
- Yazarken öneriler: ikinci karakterden itibaren önerilen komutun geri kalanı imlecin ardından soluk görünür ve → onu kabul eder. Tab kesin bir öneriyi kabul eder ya da her adayı açıklayan bir tamamlama menüsü açar, giriş alanıyla da giriş alanı olmadan da.
- Komut renkleri: komutlar, alt komutlar, seçenekler, değişkenler ve operatörler temanın renklerini alır, kabuğun çalıştıramadığı bir komutun altı ise kırmızı kesikli çizgiyle çizilir. "Ayarlar › Giriş" önerileri ve renkleri kapatır.
- Bir ajan sekmesinde alt ajan ve arka plan işlemi düğmeleri ile /tasks, her öğenin durumunu ve çalışma süresini "Göster" ve "Durdur" ile birlikte gösteren bir etkinlik görünümü açar.

## İyileştirmeler
- "Ayarlar › Genel › Pencere" altında, çalışma alanının adını adına tıklayarak değiştirme kapatılabilir.
- Kenar çubuğu değişen dosya sayısını, Git düğmesini ve klasör düğmesini bu sırayla gösterir, Git düğmesini ise yalnızca bir Git deposunun içinde.
