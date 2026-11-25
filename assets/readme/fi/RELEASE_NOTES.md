Tapa 1.0.3 avaa työtiloja muilla tietokoneilla SSH:n kautta ja ehdottaa komentoja kirjoittaessasi.

## Uutta
- Etätyötilat: ”Uusi työtila” tarjoaa kansion toisella tietokoneella: palvelimen tiedostosta `~/.ssh/config` tai osoitteen, tarvittaessa portin, käyttäjän ja salasanan sekä kansion siellä. Päätteet ja sinne asennetut agentit toimivat sillä tietokoneella yhden SSH-yhteyden kautta. Salasanan voi säilyttää avainnipussa, eikä tuntematonta palvelinavainta hyväksytä koskaan ilman vahvistusta. Tiedostoluettelo, Git-paneeli, editori ja esikatselut eivät ole siellä käytettävissä.
- Sivupalkki näyttää etätyötilan yhteyden ja toiminnot ”Yhdistä uudelleen”, ”Katkaise yhteys” ja ”Muokkaa yhteyttä”. Kun katkennut yhteys palaa, päätteet käynnistyvät uudelleen ja agenttivälilehdet jatkavat keskusteluaan.
- Ehdotukset kirjoittaessa: toisesta merkistä alkaen ehdotetun komennon loppu näkyy himmeänä kohdistimen jälkeen, ja → hyväksyy sen. Tab hyväksyy varman ehdotuksen tai avaa täydennysvalikon, joka kuvaa jokaisen vaihtoehdon, syötekentän kanssa tai ilman.
- Komentojen värit: komennot, alikomennot, valitsimet, muuttujat ja operaattorit saavat teeman värit, ja komento, jota komentotulkki ei voi suorittaa, alleviivataan punaisella katkoviivalla. ”Asetukset › Syöte” poistaa ehdotukset ja värit käytöstä.
- Agenttivälilehdellä aliagenttien ja taustaprosessien painikkeet sekä /tasks avaavat aktiviteettinäkymän, jossa näkyvät kunkin kohteen tila ja käyntiaika sekä ”Näytä” ja ”Pysäytä”.

## Parannukset
- ”Asetukset › Yleiset › Ikkuna” voi poistaa käytöstä työtilan nimeämisen uudelleen osoittamalla sen nimeä.
- Sivupalkki näyttää muuttuneiden tiedostojen määrän, Git-painikkeen ja kansiopainikkeen tässä järjestyksessä, ja Git-painikkeen vain Git-repositoriossa.
