Tapa 1.0.0, disk görüntüsü olarak çıkan ilk sürümdür: `Tapa-1.0.0.dmg` dosyasını açın ve Tapa’yı Applications klasörüne sürükleyin. Tapa 0.1.0 kendini bu sürüme günceller, Homebrew de aynı görüntüyü kurar.

## Yenilikler
- Tapa’nın son penceresini kapatmak Tapa’dan çıkar ve her şeyi korur: çalışma alanları, sekmeler, bölmeler, geçmiş ve ajan oturumları bir sonraki başlatmada bıraktığınız gibi geri gelir. Tapa yalnızca kabuk dışında bir program hâlâ çalışıyorsa önce sorar.
- Kenar çubuğundaki çalışma alanlarını istediğiniz sıraya sürükleyin; Terminal her zaman en üstte kalır.
- Yeni bir pencere, en son yeniden boyutlandırdığınız pencerenin boyutu ve kenar çubuğu genişliğiyle açılır; Tapa hiçbir pencereyi geri yüklemediğinde de bu böyledir. Tapa ayrıca giriş alanının gösterilip gösterilmediğini hatırlar.
- Tapa kenar çubuğu daraltılmış olarak başladığında Yeni Çalışma Alanı düğmesi gizli kalır.
- Bir programın art arda yazdığı çıktı, program duraklayınca en geç 8 ms sonra gösterilir; böylece yarım çizilmiş ekranlar artık titremez.
- Her oturum Tapa’nın kendi terminal çekirdeğinde çalışır; Alacritty çekirdeğine geçiren ayar kaldırıldı.
- Giriş alanının üstündeki klasör ve dal, yer tutucu metinle hizalanmış düz gri metindir.
- Bir ajan sekmesinde `/resume`, ajanın bu klasördeki önceki oturumlarını listeler ve birini kendi sekmesinde sürdürür.
- Bir ajan sekmesinin katmanındaki uzun liste, pencereyi uzatmak yerine kayar.
- Bir ajan ancak üç saniye boşta kaldıktan sonra bitmiş sayılır; böylece durumu adımlar arasında artık titremez.
- Tam ekranda kenar çubuğu araç çubuğunun altında kalır ve menü çubuğu göründüğünde onunla birlikte aşağı kayar.

## Bir bakışta Tapa
- Kenar çubuğunda çalışma alanları; her biri kendi sekmeleri, bölmeleri, ajanları, düzenleyicileri ve tarayıcı sekmeleriyle. Pencereler ve ajan oturumları yeniden başlatmadan sonra geri gelir.
- Claude, Codex, OpenCode ve kayıt defterindeki her ACP ajanı için ajan sekmeleri; ajanlar ayarlardan kurulur, güncellenir ve önceki sürüme geri alınır.
- Dosya ve Git panelleri, Markdown ve resim önizlemeleri, dikte destekli bir komut giriş alanı.
- Açık, koyu ya da sisteme uyan görünüm, dört yazı tipi boyutu, 35 arayüz dili.
