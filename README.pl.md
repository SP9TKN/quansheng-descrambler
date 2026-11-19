# Descrambler Quansheng + dekoder POCSAG (decoder_scrambler_pogsag)

> [!IMPORTANT]
> **🇵🇱 Program jest w języku polskim.** 🇬🇧 English version: [README.md](README.md)

Odwraca scrambler radiotelefonów Quansheng (UV-K5 i pochodne) i sam wykrywa jego ton.
Działa na żywo (wejście karty dźwiękowej -> wyjście) i na plikach (WAV, MP3, FLAC, OGG).

## Co obsługuje

Scrambler Quanshenga to zwykła inwersja widma: `f -> ton - f`. Ton ustawia się w menu radia,
od 2600 do 3500 Hz co 100 Hz (`BK4819_EnableScramble`: rejestr 0x71 = `0x68DC + n * 1032`).

Wykrywacz przegląda też 2000-4200 Hz co 5 Hz, więc znajdzie inwersję w innych radiach.
Przy 4096 Hz trafia rzadziej (na mowie testowej 11 z 24 przy 15 s), resztę zgłasza jako niepewną.
Nie obsługuje inwersji dzielonej (split-band), kroczącej nośnej ani radia cyfrowego;
dla takich sygnałów ma odpowiedzieć „niepewny”.

Obok działa dekoder pagerów POCSAG 512/1200/2400 bit/s, na żywo z tego samego wejścia
i z plików. Pokazuje RIC odbiorcy, funkcję, treść jako tekst i jako cyfry oraz poprawione bity.
Numeru nadawcy POCSAG nie przesyła. Wiadomości odebrane na żywo trafiają też do pliku dnia
`POCSAG_RRRR-MM-DD.txt` w folderze programu (UTF-8, linia na wiadomość, pola rozdzielone `|`);
wiadomości z plików nie, bo to stare nagrania.

## Użycie

`decoder_scrambler_pogsag.exe`: wybierz wejście z odbiornika i wyjście na słuchawki, naciśnij Start.
Automat potrzebuje kilku sekund mowy. Paski pokazują punkty tonów co 50 Hz, kliknięcie tonu ustawia go ręcznie.
„Dowolny ton” z polem obok ustawia ręcznie każdą nośną z zakresu skanu, także spoza listy Quansheng.
Urządzenia są zapamiętane w `%APPDATA%\descrambler-quansheng.txt`.

    descrambler-cli wykryj <plik>
    descrambler-cli odwroc <plik> <wyj.wav> [--ton HZ]
    descrambler-cli symuluj <plik>
    descrambler-cli pocsag <plik>
    descrambler-cli urzadzenia
    descrambler-cli na-zywo <wejście> <wyjście> <sekundy>

## Jak działa wykrywanie

Harmoniczne głosu leżą na `k*F0`, a po inwersji na `ton - k*F0`. To grzebień o odstępie F0,
zaczepiony w 0 albo w tonie. Faza grzebienia w każdej ramce mówi, gdzie jest zaczep (modulo F0),
a zmienność F0 między ramkami usuwa tę wieloznaczność. Średnie widmo jest odejmowane, więc
CTCSS i stałe tony nie przeszkadzają; obwiednia mowy tylko rozstrzyga remisy.

Słaby punkt: mowa o prawie stałym F0 nie odróżnia nośnej od nośnej ± F0 (te same fazy
grzebienia). Ramki 64 ms idą co 16 ms; przy kroku 32 ms przesunięcie początku nagrania
o kilka milisekund zamieniało taki remis w „pewny” wynik jednej strony. Przy remisie plik
dostaje wynik niepewny, a komunikat podaje obu kandydatów.

Ramki z mniej niż 4 szczytami widma są odrzucane, bo to nie głos: beep z tonów co 100 Hz
pasowałby do grzebienia zaczepionego w każdym tonie Quansheng.

Na żywo analiza idzie co 0,25 s, a ton zmienia się po 3 zgodnych wynikach. Ton zostaje między
nadaniami, więc kolejne nadanie tej samej stacji jest odwracane od początku. Cisza od 0,6 s
kasuje pamięć mowy, żeby nie mieszać stacji. Tony spoza listy wykryte w sesji są zapamiętywane
i potem rozpoznawane tak szybko jak tony z listy.

## Jak działa POCSAG

Tor audio radia nie przenosi niskich częstotliwości (filtr CTCSS ok. 250 Hz) i robi deemfazę,
więc znak sygnału daje 2-4 złe bity na słowo, a BCH poprawia tylko 2. Zegar bitów idzie z linii
widma kwadratu pochodnej (faza wygładzana w obie strony, trzy stałe czasowe do wyboru). Decyzje
daje korektor liniowy (zaczepy co 1/8 bitu, +-3 bity), uczony najmniejszymi kwadratami na słowach
o znanej treści: sync, idle, słowa poprawne według BCH. Kilka przebiegów, każdy na wyniku
poprzedniego, więc korektor dopasowuje się do radia sam.

Paczka liczy się dopiero, gdy połowa jej słów (min. 5) jest poprawna: przypadkowy sync w mowie
po nauce korektora wygląda idealnie, a jego słowa nie. Idle jest rozpoznawany przed poprawą BCH,
bo poprawa 2 bitów robi z idle z 4 błędami inne słowo; słowo z samych zer (cisza) jest odrzucane.

Na żywo okno 8 s jest dekodowane co sekundę (ok. 75 ms pracy); wiadomość wychodzi raz, gdy jest
cała. Ograniczenie: 2400 bit/s po deemfazie gubi koniec tekstu, bo długie ciągi zer giną
w filtrze górnoprzepustowym; do 2400 potrzebny płaski tor (SDR, wyjście dyskryminatora).

## Budowanie

Toolchain `x86_64-pc-windows-gnu`, a w `PATH` musi być `C:\msys64\mingw64\bin` (dlltool):

    cargo test --release --lib
    cargo build --release --bins

## Testy

- `cargo test`: inwerter, wykrywacz i automat na sygnale syntetycznym; POCSAG: koder, tor radia
  (filtr górnoprzepustowy, deemfaza, odwrócona polaryzacja, szum) i dekodowanie na żywo bez powtórzeń.
- `examples/pocsag_czas.rs <nagranie>`: czas dekodowania okna 8 s dla każdej prędkości.
- `testy/lawa.py <katalog_mowy> <cli>`: mowa -> szyfrowanie w scipy (niezależna implementacja) -> wykrywanie.
- `testy/symuluj_wszyscy.py <katalog_mowy> <cli>`: automat na całych nagraniach; czy kiedyś przeskoczy na zły ton.
- `testy/weryfikuj.py <katalog_mowy> <cli>`: korelacja odwróconej mowy z oryginałem.
- `testy/decyzje.py <cli> <nagranie>`: zmiany decyzji automatu na nagraniu, z czasem trwania.
- `testy/wytnij_tony.py <folder> <nagranie>...`: wycina nadania z sygnałami tonowymi (beepy, alarmy).
- `examples/petla.rs`: odtwarzanie i nagrywanie przez kable wirtualne, do testu na żywo.
- `examples/przebieg.rs`: zrzut wszystkich analiz do CSV, do strojenia reguł offline.

Mowa testowa: Open Speech Repository, `OSR_us_000_0010_8k`, `0011`, `0030`, `OSR_uk_000_0020_8k`.

## Podziękowania

Tor przetwarzania i pomysł na cechę wykrywania pochodzą z
[deinvert](https://github.com/windytan/deinvert) (Oona Räisänen, MIT); kod jest napisany od nowa.

## Poprawka 03.11.2026: zgubione próbki (przeskok)

Karta albo kabel wirtualny pod obciążeniem gubi czasem cały pakiet (10 ms, przy 512 bit/s 5,25 bitu).
Reszta nadania się przesuwa. Dekoder przedłużał wtedy łańcuch paczek na przesuniętą pozycję,
gdzie przesunięte słowa idle wyglądają jak adresy: powstawała seria fałszywych RIC co ramkę, a paczka
za przeskokiem przepadała, bo zaczynała się 539 bitów po poprzedniej i liczyła się jako nakładka.

Test `przeskok_bez_falszywych_adresow` (10 ms wycinane po kolei w różnych miejscach nadania):

| | fałszywe RIC przed | po | druga wiadomość cała przed | po |
|---|---|---|---|---|
| 512 bit/s, 81 prób | 490 | 1 | 49 | 54 |
| 1200 bit/s, 31 prób | 210 | 0 | 17 | 19 |

Reguły: łańcuch kończy się na paczce przesuniętej o kilka bitów i przy dobrym syncu do 32 bitów obok;
paczki nachodzą dopiero bliżej niż 544 - 32 bity; słowo bliskie idle przesuniętemu (78 z 2^21 słów
kodowych) to idle przesunięte; w paczce przed przesuniętą reszta od pierwszego niepewnego słowa to
śmieci; adres przed idle przesuniętym to koniec wiadomości. Ta sama poprawka jest w
[pocsag-encoder-decoder](https://github.com/SP9TKN/pocsag-encoder-decoder) (koder + dekoder).
Nie sprawdzona na prawdziwych nagraniach pagera.

## Poprawka 05.11.2026: korektor ze sprzężeniem i sito na szum

Ten sam silnik co w [pocsag-encoder-decoder](https://github.com/SP9TKN/pocsag-encoder-decoder) (opis i pomiary w jego README).

- Korektor dostał sprzężenie od własnych decyzji (poprzedni bit i zanikający ślad dawniejszych).
  Filtr górnoprzepustowy radia zostawia z długiego ciągu tych samych bitów tylko zbocza, a takie
  ciągi są w ostatnim słowie tekstu (dopełnienie zerami) i w adresach małych RIC. Przy szumie tekst
  dochodził cały w 17/30 (512 bit/s) i 10/30 (1200), teraz w 30/30 i 29/30. Działają też deemfaza przy
  2400 bit/s i małe RIC.
- Łańcuch paczek nie dokleja paczek z szumu po końcu nadania (bez dobrego syncu potrzeba 4 pewnych
  słów). Sito odrzuca wiadomości z ponad połową słów nie do poprawienia i adres poprawiony na 2 bitach
  bez czystej treści. Fałszywe RIC z szumu po nadaniu: przed 3-12 na 40 prób, po 0.
- Testy: `szum_po_nadaniu_bez_falszywych`, 2400 bit/s po deemfazie w `trzy_predkosci_przez_tor_radia`;
  przeskok: 0 fałszywych RIC (było 1 na 81).

## Licencja

[GPL-3.0](LICENSE) © 2026 SP9TKN.

**Narzędzia:** program powstał z pomocą [Claude Code](https://claude.com/claude-code) (Anthropic) — asystent AI do pisania kodu, testów i dokumentacji.
