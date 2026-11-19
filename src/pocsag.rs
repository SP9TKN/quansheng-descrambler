//! Dekoder POCSAG 512/1200/2400 bit/s z wyjścia audio odbiornika FM.
//!
//! Tor audio radia nie przenosi niskich częstotliwości (filtr CTCSS, ok. 250 Hz) i robi deemfazę,
//! więc sam znak sygnału daje 2-4 złe bity na słowo, a BCH poprawia tylko 2. Zegar bitów idzie
//! z linii widma kwadratu pochodnej, a decyzje z korektora liniowego uczonego metodą najmniejszych
//! kwadratów na słowach o znanej treści: sync, idle i słowa poprawne według BCH. Każdy przebieg
//! uczy się na wyniku poprzedniego, więc korektor sam dopasowuje się do toru audio radia.

use std::collections::VecDeque;
use std::f64::consts::PI;
use std::sync::OnceLock;

pub const PREDKOSCI: [u32; 3] = [512, 1200, 2400];

pub(crate) const SYNC: u32 = 0x7CD2_15D8;
pub(crate) const IDLE: u32 = 0x7A89_C197;
pub(crate) const SLOW_W_PACZCE: usize = 16;
const BITOW_PACZKI: usize = 32 * (SLOW_W_PACZCE + 1);
/// Stałe czasowe wygładzania zegara w bitach. Krótka goni treść danych, długa nie nadąża
/// za różnicą zegarów; wygrywa ta, która daje najwięcej poprawnych słów.
const STALE_ZEGARA: [f64; 3] = [80.0, 150.0, 280.0];
/// Korektor: zaczepy co 1/8 bitu w zasięgu +-3 bity, plus stała.
const ZACZEPY_NA_BIT: usize = 8;
const ZASIEG_BITOW: usize = 3;
const SRODEK: usize = ZACZEPY_NA_BIT * ZASIEG_BITOW;
const CECH: usize = 2 * SRODEK + 2;
/// Sprzężenie od decyzji: poprzedni bit i zanikający ślad dawniejszych (stała SLAD_BITOW).
/// Filtr górnoprzepustowy radia zostawia z długiego ciągu tych samych bitów tylko zbocza, więc
/// bit w środku ciągu wynika z poprzednich decyzji, a nie z próbek wokół niego. Bez tego ostatnie
/// słowo tekstu, dopełnione zerami, miało średnio 5-6 złych bitów (pozostałe poniżej 1).
const SPRZEZENIE: usize = 2;
const CECH_DFE: usize = CECH + SPRZEZENIE;
const SLAD_BITOW: f32 = 4.0;
const PRZEBIEGOW: usize = 6;
const MAX_TRENINGU: usize = 2000;
/// Mniej znanych bitów to zwykle przypadkowy „sync” w szumie: nie ma czego uczyć.
const MIN_TRENINGU: usize = 128;
/// Po filtrze wystarczy tyle próbek na bit; dalsze kroki idą na rzadszych próbkach.
const PROBEK_NA_BIT: f64 = 16.0;
/// Sync z tyloma błędami jest tylko kandydatem: paczkę potwierdzają jej poprawne słowa.
const BLEDY_SYNC: u32 = 6;
/// Paczka doklejona do łańcucha bez dobrego syncu musi mieć tyle pewnych słów (w szumie
/// zdarza się to raz na kilka tysięcy paczek).
const MIN_PEWNYCH_BEZ_SYNCU: usize = 4;
/// Na żywo: wiadomość musi się zmieścić w oknie razem z syncem swojej paczki.
const OKNO_S: f32 = 8.0;
const SYGNAL_RMS: f32 = 0.003;

#[derive(Clone, Debug, PartialEq)]
pub struct Wiadomosc {
    pub predkosc: u32,
    /// Numer pagera odbiorcy (capcode). Nadawcy POCSAG nie przesyła.
    pub ric: u32,
    /// Bity funkcji 0-3: rodzaj sygnału albo podadres w pagerze.
    pub funkcja: u8,
    /// Słowa treści po 20 bitów; puste przy samym wywołaniu.
    pub dane: Vec<u32>,
    /// Bity poprawione przez BCH w adresie i treści.
    pub poprawki: u32,
    /// Słowa treści, których BCH nie poprawił: ich znaki mogą być złe.
    pub bledne_slowa: usize,
    /// Próbka początku słowa adresu.
    pub probka: usize,
    /// false: dane skończyły się w środku wiadomości, może mieć dalszy ciąg.
    pub zakonczona: bool,
}

impl Wiadomosc {
    fn bity(&self) -> Vec<u8> {
        self.dane.iter().flat_map(|&d| (0..20).rev().map(move |i| ((d >> i) & 1) as u8)).collect()
    }

    fn znaki(&self) -> Vec<u8> {
        let mut wyj = Vec::new();
        for z in self.bity().chunks_exact(7) {
            // Znaki 7-bitowe, najmłodszy bit pierwszy.
            let c = z.iter().enumerate().fold(0u8, |a, (j, &v)| a | (v << j));
            if matches!(c, 3 | 4 | 0x17) {
                break;
            }
            wyj.push(c);
        }
        wyj
    }

    /// Treść jako tekst; koniec na ETX/EOT/ETB, znaki sterujące jako kropka.
    pub fn tekst(&self) -> String {
        let s: String = self
            .znaki()
            .into_iter()
            .filter(|&c| c != 0)
            .map(|c| match c {
                0x0A | 0x0D => ' ',
                32..=126 => c as char,
                _ => '·',
            })
            .collect();
        s.trim_end().to_string()
    }

    /// Treść jako cyfry: 4 bity na znak, najmłodszy pierwszy.
    pub fn cyfry(&self) -> String {
        const ZNAKI: &[u8; 16] = b"0123456789*U -)(";
        let s: String = self
            .bity()
            .chunks_exact(4)
            .map(|z| ZNAKI[(z[0] | z[1] << 1 | z[2] << 2 | z[3] << 3) as usize] as char)
            .collect();
        s.trim_end().to_string()
    }

    /// POCSAG nie mówi, czy treść to tekst, czy cyfry (funkcja bywa używana różnie), więc z treści.
    pub fn wyglada_na_tekst(&self) -> bool {
        let z: Vec<u8> = self.znaki().into_iter().filter(|&c| c != 0).collect();
        let drukowalne = z.iter().filter(|c| (32..=126).contains(*c)).count();
        !z.is_empty() && drukowalne * 100 >= z.len() * 85 && z.iter().any(u8::is_ascii_alphabetic)
    }

    /// Główny odczyt treści i drugi, z przedrostkiem „cyfry:” albo „tekst:”.
    pub fn odczyty(&self) -> (String, Option<String>) {
        if self.dane.is_empty() {
            ("(samo wywołanie, bez treści)".to_string(), None)
        } else if self.wyglada_na_tekst() {
            (self.tekst(), Some(format!("cyfry: {}", self.cyfry())))
        } else {
            (self.cyfry(), Some(format!("tekst: {}", self.tekst())))
        }
    }

    pub fn jakosc(&self) -> String {
        let mut s = format!("poprawione bity: {}", self.poprawki);
        if self.bledne_slowa > 0 {
            s += &format!(", słowa z błędem: {} (znaki mogą być złe)", self.bledne_slowa);
        }
        if !self.zakonczona {
            s += ", urwana";
        }
        s
    }
}

// ---------- BCH(31,21) + bit parzystości ----------

pub(crate) fn syndrom(v: u32) -> u32 {
    let mut v = v;
    for i in (10..=30).rev() {
        if v & (1 << i) != 0 {
            v ^= 0x769 << (i - 10);
        }
    }
    v
}

/// Wzorzec błędu (1 albo 2 bity z 31) dla każdego syndromu.
fn tablica_bledow() -> &'static [u32] {
    static T: OnceLock<Vec<u32>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![0u32; 1024];
        for i in 0..31 {
            for j in i..31 {
                let e = (1u32 << i) | (1u32 << j);
                let s = syndrom(e) as usize;
                if t[s] == 0 || e.count_ones() < t[s].count_ones() {
                    t[s] = e;
                }
            }
        }
        t
    })
}

/// Poprawia do 2 bitów; zwraca słowo i liczbę poprawionych bitów.
pub(crate) fn popraw(cw: u32) -> Option<(u32, u32)> {
    let parzyste = |c: u32| c.count_ones() % 2 == 0;
    let s = syndrom(cw >> 1) as usize;
    if s == 0 {
        return Some(if parzyste(cw) { (cw, 0) } else { (cw ^ 1, 1) });
    }
    let e = tablica_bledow()[s];
    if e == 0 {
        return None;
    }
    let c = cw ^ (e << 1);
    match (parzyste(c), e.count_ones()) {
        (true, n) => Some((c, n)),
        (false, 1) => Some((c ^ 1, 2)),
        _ => None,
    }
}

/// Okna z ciągu słów idle przesunięte o 1-31 bitów; na końcu nadania dalej jest cisza,
/// więc także idle dosunięte zerami albo jedynkami, ale tylko do 16 bitów: przy większym
/// przesunięciu zostaje kilka jedynek, a do tego jest blisko cisza i adresy małych RIC.
fn idle_przesuniete() -> &'static [u32] {
    static T: OnceLock<Vec<u32>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t: Vec<u32> = (1..32u32).map(|k| IDLE.rotate_left(k)).collect();
        t.extend((1..=16u32).flat_map(|k| [IDLE << k, (IDLE << k) | ((1u32 << k) - 1)]));
        t
    })
}

/// Znacznik idle przesuniętego (na miejscu liczby poprawionych bitów): odróżnia je od idle
/// z 3 błędami i nigdy nie jest „pewne”.
const IDLE_PRZESUNIETE: u32 = 4;

/// Idle sprawdzany najpierw: poprawa 2 bitów robi z idle z 4 błędami inne, fałszywe słowo.
fn rozpoznaj(raw: u32) -> Option<(u32, u32)> {
    let d = (raw ^ IDLE).count_ones();
    if d <= 3 {
        return Some((IDLE, d));
    }
    // Idle przesunięte o kilka bitów (karta zgubiła próbki): bywa „poprawnym” adresem i daje
    // serię fałszywych RIC co ramkę. Liczy się jako idle, ale niepewne (nie potwierdza paczki).
    // Prawdziwe słowo kodowe wpada tu z prawdopodobieństwem ok. 8e-5.
    if idle_przesuniete().iter().any(|&w| (raw ^ w).count_ones() <= 3) {
        return Some((IDLE, IDLE_PRZESUNIETE));
    }
    // Cisza daje same zera, a to formalnie poprawne słowo adresu.
    popraw(raw).filter(|&(c, _)| c != 0 && c != u32::MAX)
}

fn pewne(r: Option<(u32, u32)>) -> bool {
    matches!(r, Some((c, n)) if n <= if c == IDLE { 2 } else { 1 })
}

// ---------- ramkowanie ----------

fn slowo(b: &[u8], p: usize) -> u32 {
    b[p..p + 32].iter().fold(0, |w, &x| (w << 1) | x as u32)
}

struct Paczka {
    poz: usize,
    odwr: bool,
    /// Słowo po odwróceniu polaryzacji i wynik rozpoznania.
    slowa: Vec<(u32, Option<(u32, u32)>)>,
    pewnych: usize,
}

fn paczka(b: &[u8], poz: usize, odwr: bool) -> Paczka {
    let mut slowa = Vec::new();
    for m in 0..SLOW_W_PACZCE {
        let p = poz + 32 * (m + 1);
        if p + 32 > b.len() {
            break;
        }
        let raw = slowo(b, p) ^ if odwr { u32::MAX } else { 0 };
        slowa.push((raw, rozpoznaj(raw)));
    }
    let pewnych = slowa.iter().filter(|(_, r)| pewne(*r)).count();
    Paczka { poz, odwr, slowa, pewnych }
}

fn bledy_sync(b: &[u8], poz: usize, odwr: bool) -> u32 {
    if poz + 32 > b.len() {
        return 99;
    }
    (slowo(b, poz) ^ SYNC ^ if odwr { u32::MAX } else { 0 }).count_ones()
}

/// Kandydaci na sync w obu polaryzacjach, najlepsi pierwsi.
fn kandydaci(b: &[u8]) -> Vec<(usize, bool, u32)> {
    let mut k = Vec::new();
    let mut s = 0u32;
    for (i, &x) in b.iter().enumerate() {
        s = (s << 1) | x as u32;
        if i >= 31 {
            let e = (s ^ SYNC).count_ones();
            if e <= BLEDY_SYNC {
                k.push((i - 31, false, e));
            }
            if 32 - e <= BLEDY_SYNC {
                k.push((i - 31, true, 32 - e));
            }
        }
    }
    k.sort_by_key(|c| c.2);
    k
}

fn najlepszy_sync(b: &[u8]) -> u32 {
    let mut s = 0u32;
    let mut naj = 32;
    for (i, &x) in b.iter().enumerate() {
        s = (s << 1) | x as u32;
        if i >= 31 {
            let e = (s ^ SYNC).count_ones();
            naj = naj.min(e).min(32 - e);
        }
    }
    naj
}

/// Karta, która zgubiła próbki, skraca odstęp paczek: następna zaczyna się kilka bitów wcześniej.
const MAX_PRZESKOK: usize = 32;

/// Paczki bliżej niż 544 - 32 bity nachodzą na siebie; bliżej o mniej niż 32 to przeskok, a nie
/// nakładka (inaczej paczka za przeskokiem przepada razem ze swoimi wiadomościami).
fn nachodzi(lista: &[Paczka], poz: usize) -> bool {
    lista.iter().any(|p| p.poz.abs_diff(poz) < BITOW_PACZKI - MAX_PRZESKOK)
}

/// Słaby sync w miejscu łańcucha, a dobry kilka bitów obok: nadanie się przesunęło.
fn lepszy_sync_obok(b: &[u8], poz: usize, odwr: bool) -> bool {
    bledy_sync(b, poz, odwr) > BLEDY_SYNC
        && (1..=MAX_PRZESKOK).any(|d| bledy_sync(b, poz + d, odwr) <= BLEDY_SYNC || (poz >= d && bledy_sync(b, poz - d, odwr) <= BLEDY_SYNC))
}

/// Do nauki: każdy kandydat na sync, bez potwierdzania.
fn paczki_luzne(b: &[u8]) -> Vec<Paczka> {
    let mut wyj: Vec<Paczka> = Vec::new();
    for (poz, odwr, _) in kandydaci(b) {
        if !nachodzi(&wyj, poz) {
            wyj.push(paczka(b, poz, odwr));
        }
    }
    wyj
}

/// Paczki potwierdzone połową poprawnych słów, a od nich łańcuch co 544 bity.
fn paczki_pewne(b: &[u8]) -> Vec<Paczka> {
    let mut wyj: Vec<Paczka> = Vec::new();
    for (poz, odwr, _) in kandydaci(b) {
        if nachodzi(&wyj, poz) {
            continue;
        }
        let p = paczka(b, poz, odwr);
        if p.slowa.len() >= 5 && p.pewnych >= 5.max(p.slowa.len() / 2) {
            wyj.push(p);
        }
    }
    let poczatki: Vec<(usize, bool)> = wyj.iter().map(|p| (p.poz, p.odwr)).collect();
    for (p0, odwr) in poczatki {
        let mut poz = p0 + BITOW_PACZKI;
        // Łańcuch kończy się na innej paczce, także przesuniętej o kilka bitów: karta, która
        // zgubiła kilka ms próbek, przesuwa resztę nadania, a przesunięte słowa idle wyglądają
        // jak adresy (seria fałszywych RIC co ramkę).
        while poz + 64 <= b.len() && !nachodzi(&wyj, poz) && !lepszy_sync_obok(b, poz, odwr) {
            let p = paczka(b, poz, odwr);
            // Dalej tylko paczka, która wygląda na POCSAG: dobry sync i choć jedno pewne słowo
            // albo kilka pewnych słów. Szum po końcu nadania (otwarta blokada szumów) ma słaby
            // sync i pojedyncze przypadkowe „pewne” słowa, a z nich były fałszywe adresy.
            let dobry_sync = bledy_sync(b, poz, odwr) <= BLEDY_SYNC;
            if !(dobry_sync && p.pewnych >= 1 || p.pewnych >= MIN_PEWNYCH_BEZ_SYNCU) {
                break;
            }
            wyj.push(p);
            poz += BITOW_PACZKI;
        }
        // Wstecz tylko z dobrym syncem: przed pierwszą paczką jest preambuła.
        let mut poz = p0;
        while poz >= BITOW_PACZKI {
            poz -= BITOW_PACZKI;
            if nachodzi(&wyj, poz) || bledy_sync(b, poz, odwr) > BLEDY_SYNC {
                break;
            }
            wyj.push(paczka(b, poz, odwr));
        }
    }
    wyj.sort_by_key(|p| p.poz);
    wyj
}

struct Surowa {
    bit: usize,
    ric: u32,
    funkcja: u8,
    dane: Vec<u32>,
    poprawki: u32,
    /// Bity poprawione w samym słowie adresu.
    poprawki_adresu: u32,
    bledne: usize,
    zakonczona: bool,
}

impl Surowa {
    /// Sito na odczyty z szumu: więcej niż połowa słów treści nie do poprawienia to nie
    /// wiadomość; adres poprawiony na 2 bitach (granica BCH, w szumie co czwarte słowo) przechodzi
    /// tylko z treścią bez błędów, która go potwierdza.
    fn wiarygodna(&self) -> bool {
        let n = self.dane.len();
        if self.bledne * 2 > n {
            return false;
        }
        !(self.poprawki_adresu >= 2 && (n == 0 || self.bledne > 0))
    }
}

/// Składa wiadomości: adres, potem słowa treści aż do idle albo następnego adresu.
fn zloz(paczki: &[Paczka], bitow: usize) -> Vec<Surowa> {
    let mut wyj = Vec::new();
    let mut biezaca: Option<Surowa> = None;
    let mut poprzednia: Option<usize> = None;
    for (i, p) in paczki.iter().enumerate() {
        if poprzednia.is_some_and(|q| p.poz != q + BITOW_PACZKI) {
            wyj.extend(biezaca.take());
        }
        poprzednia = Some(p.poz);
        // Następna paczka kilka bitów obok miejsca co 544: w tej zginęły próbki, a słowa za tym
        // miejscem są przesunięte. Od pierwszego niepewnego słowa reszta paczki to śmieci.
        let przeskok = paczki.get(i + 1).is_some_and(|q| {
            let d = q.poz.abs_diff(p.poz + BITOW_PACZKI);
            d != 0 && d <= MAX_PRZESKOK
        });
        let mut zepsute = false;
        for (m, &(raw, r)) in p.slowa.iter().enumerate() {
            zepsute |= przeskok && !pewne(r);
            // Adres, za którym idzie przesunięte idle, leży na styku przeskoku (ostatnia paczka
            // nadania nie ma następnej, po której przeskok byłby widać): to koniec wiadomości.
            let za_nim_przesuniete =
                p.slowa.get(m + 1).is_some_and(|&(_, r2)| r2 == Some((IDLE, IDLE_PRZESUNIETE)));
            let r = if zepsute {
                None
            } else if za_nim_przesuniete && matches!(r, Some((c, _)) if c != IDLE && c >> 31 == 0) {
                Some((IDLE, IDLE_PRZESUNIETE))
            } else {
                r
            };
            match r {
                Some((IDLE, _)) => wyj.extend(biezaca.take()),
                Some((c, n)) if c >> 31 == 0 => {
                    wyj.extend(biezaca.take());
                    biezaca = Some(Surowa {
                        bit: p.poz + 32 * (m + 1),
                        // Pozycja w paczce (ramka) to 3 najmłodsze bity numeru.
                        ric: (((c >> 13) & 0x3FFFF) << 3) | (m / 2) as u32,
                        funkcja: ((c >> 11) & 3) as u8,
                        dane: Vec::new(),
                        poprawki: n,
                        poprawki_adresu: n,
                        bledne: 0,
                        zakonczona: true,
                    });
                }
                Some((c, n)) => {
                    if let Some(w) = &mut biezaca {
                        w.dane.push((c >> 11) & 0xFFFFF);
                        w.poprawki += n;
                    }
                }
                None => {
                    if let Some(w) = &mut biezaca {
                        w.dane.push((raw >> 11) & 0xFFFFF);
                        w.bledne += 1;
                    }
                }
            }
        }
    }
    if let Some(mut w) = biezaca {
        // Łańcuch urwał się na końcu danych, a nie na końcu nadania.
        let ostatnia = paczki.last().unwrap();
        w.zakonczona = ostatnia.slowa.len() == SLOW_W_PACZCE && ostatnia.poz + BITOW_PACZKI + 64 <= bitow;
        wyj.push(w);
    }
    wyj
}

// ---------- tor sygnału ----------

/// Butterworth 4. rzędu, w przód i w tył (bez przesunięcia fazy).
fn dolnoprzepustowy(x: &[f32], fs: f64, fc: f64) -> Vec<f64> {
    // Stała 1e-20 trzyma stan filtru z dala od liczb subnormalnych: w ciszy z cyfrowych zer
    // utyka w nich i liczy kilkanaście razy wolniej. Korektor ma wyraz stały, więc to bez znaczenia.
    let mut y: Vec<f64> = x.iter().map(|&v| v as f64 + 1e-20).collect();
    let w = 2.0 * PI * fc / fs;
    let (sn, cs) = w.sin_cos();
    let filtruj = |y: &mut Vec<f64>| {
        for q in [0.541_196_1, 1.306_563] {
            let al = sn / (2.0 * q);
            let a0 = 1.0 + al;
            let b0 = (1.0 - cs) / 2.0 / a0;
            let b1 = (1.0 - cs) / a0;
            let a1 = -2.0 * cs / a0;
            let a2 = (1.0 - al) / a0;
            let (mut z1, mut z2) = (0.0, 0.0);
            for v in y.iter_mut() {
                let o = b0 * *v + z1;
                z1 = b1 * *v - a1 * o + z2;
                z2 = b0 * *v - a2 * o;
                *v = o;
            }
        }
    };
    filtruj(&mut y);
    y.reverse();
    filtruj(&mut y);
    y.reverse();
    y
}

/// Środki bitów w próbkach (ułamkowo). Kwadrat pochodnej ma szczyty na zmianach bitów, więc
/// linię na częstotliwości bitowej; jej faza, wygładzona w obie strony, wyznacza zegar.
/// Wygładzona faza zmienia się wolno, więc wystarczą bloki, a chwile między nimi daje interpolacja.
fn chwile_bitow(bloki: &[(f64, f64)], blok: usize, w0: f64, a: f64, probek: f64, margines: f64) -> Vec<f64> {
    let n = bloki.len();
    let mut wyj = Vec::new();
    if n < 2 {
        return wyj;
    }
    let mut s = vec![(0.0, 0.0); n];
    let (mut re, mut im) = (0.0, 0.0);
    for i in 0..n {
        re = a * re + (1.0 - a) * bloki[i].0;
        im = a * im + (1.0 - a) * bloki[i].1;
        s[i] = (re, im);
    }
    let (mut re, mut im) = (0.0, 0.0);
    for i in (0..n).rev() {
        re = a * re + (1.0 - a) * s[i].0;
        im = a * im + (1.0 - a) * s[i].1;
        s[i] = (re, im);
    }
    let srodek = |k: usize| (k * blok) as f64 + (blok as f64 - 1.0) / 2.0;
    let mut poprz_kat = s[0].1.atan2(s[0].0);
    let mut faza = poprz_kat;
    // Faza = w0*(t - zmiana bitu); środek bitu wypada w pi + 2*pi*k.
    let (mut t_maks, mut maks) = (srodek(0), w0 * srodek(0) + faza);
    let mut cel = PI + 2.0 * PI * ((maks - PI) / (2.0 * PI)).ceil();
    for (k, &(re, im)) in s.iter().enumerate().skip(1) {
        let kat = im.atan2(re);
        let mut d = kat - poprz_kat;
        if d > PI {
            d -= 2.0 * PI;
        } else if d < -PI {
            d += 2.0 * PI;
        }
        poprz_kat = kat;
        faza += d;
        let t = srodek(k);
        let psi = w0 * t + faza;
        if psi > maks {
            while psi >= cel {
                let tc = t_maks + (cel - maks) / (psi - maks) * (t - t_maks);
                if tc >= margines && tc < probek - margines {
                    wyj.push(tc);
                }
                cel += 2.0 * PI;
            }
            (t_maks, maks) = (t, psi);
        }
    }
    wyj
}

/// Wiersz cech: próbki wokół środka bitu co 1/8 bitu i stała.
fn cechy(y: &[f64], t: &[f64], spb: f64) -> Vec<f32> {
    let mut f = Vec::with_capacity(t.len() * CECH);
    for &tk in t {
        for j in 0..=2 * SRODEK {
            let p = tk + (j as f64 - SRODEK as f64) * spb / ZACZEPY_NA_BIT as f64;
            let i = p.floor() as usize;
            let u = p - i as f64;
            f.push((y[i] * (1.0 - u) + y[i + 1] * u) as f32);
        }
        f.push(1.0);
    }
    f
}

/// Najmniejsze kwadraty z lekką regularyzacją; None przy osobliwym układzie.
fn naucz(f: &[f32], n: usize, trening: &[(usize, f32)]) -> Option<Vec<f32>> {
    let krok = trening.len().div_ceil(MAX_TRENINGU).max(1);
    let mut ata = vec![0.0f64; n * n];
    let mut atd = vec![0.0f64; n];
    for &(k, d) in trening.iter().step_by(krok) {
        let w = &f[k * n..(k + 1) * n];
        for i in 0..n {
            let wi = w[i] as f64;
            atd[i] += wi * d as f64;
            for j in i..n {
                ata[i * n + j] += wi * w[j] as f64;
            }
        }
    }
    let slad: f64 = (0..n).map(|i| ata[i * n + i]).sum();
    for i in 0..n {
        ata[i * n + i] += 1e-3 * slad / n as f64;
        for j in 0..i {
            ata[i * n + j] = ata[j * n + i];
        }
    }
    // Cholesky: A = L L^T.
    let mut l = vec![0.0f64; n * n];
    for i in 0..n {
        for j in 0..=i {
            let s: f64 = ata[i * n + j] - (0..j).map(|k| l[i * n + k] * l[j * n + k]).sum::<f64>();
            if i == j {
                if s <= 0.0 {
                    return None;
                }
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    let mut z = vec![0.0f64; n];
    for i in 0..n {
        z[i] = (atd[i] - (0..i).map(|k| l[i * n + k] * z[k]).sum::<f64>()) / l[i * n + i];
    }
    let mut x = vec![0.0f64; n];
    for i in (0..n).rev() {
        x[i] = (z[i] - (i + 1..n).map(|k| l[k * n + i] * x[k]).sum::<f64>()) / l[i * n + i];
    }
    Some(x.into_iter().map(|v| v as f32).collect())
}


/// Wiersze cech ze sprzężeniem, do nauki: przeszłe bity z decyzji, a tam, gdzie bit jest znany
/// (sync, idle, poprawne słowa), z wartości znanej.
fn wiersze_dfe(f: &[f32], b: &[u8], trening: &[(usize, f32)]) -> Vec<f32> {
    let mut x: Vec<f32> = b.iter().map(|&v| if v == 1 { 1.0 } else { -1.0 }).collect();
    for &(k, d) in trening {
        x[k] = d;
    }
    let a = (-1.0 / SLAD_BITOW).exp();
    let (mut pop, mut slad) = (0.0f32, 0.0f32);
    let mut wyj = Vec::with_capacity(b.len() * CECH_DFE);
    for (k, r) in f.chunks_exact(CECH).enumerate() {
        wyj.extend_from_slice(r);
        wyj.push(pop);
        wyj.push(slad);
        slad = a * (slad + pop);
        pop = x[k];
    }
    wyj
}

/// Decyzje po kolei: sprzężenie bierze własne poprzednie decyzje.
fn decyzje_dfe(f: &[f32], w: &[f32]) -> Vec<u8> {
    let a = (-1.0 / SLAD_BITOW).exp();
    let (mut pop, mut slad) = (0.0f32, 0.0f32);
    f.chunks_exact(CECH)
        .map(|r| {
            let v = r.iter().zip(&w[..CECH]).map(|(p, q)| p * q).sum::<f32>() + w[CECH] * pop + w[CECH + 1] * slad;
            let bit = v > 0.0;
            slad = a * (slad + pop);
            pop = if bit { 1.0 } else { -1.0 };
            bit as u8
        })
        .collect()
}

/// Bity słowa o znanej treści jako cele nauki (+1/-1).
fn dodaj_cele(trening: &mut Vec<(usize, f32)>, poz: usize, slowo: u32, odwr: bool) {
    for i in 0..32 {
        let bit = ((slowo >> (31 - i)) & 1 == 1) != odwr;
        trening.push((poz + i, if bit { 1.0 } else { -1.0 }));
    }
}

/// Przebiegi korektora; zwraca liczbę pewnych słów i paczki z najlepszego przebiegu.
fn ucz_i_ramkuj(f: &[f32]) -> Option<(usize, Vec<Paczka>)> {
    // Start: jeden zaczep tam, gdzie sync wypada najlepiej, bo tor radia przesuwa środek bitu.
    let (bledy, zaczep) = (SRODEK - 4..=SRODEK + 4)
        .map(|z| {
            let b: Vec<u8> = f.chunks_exact(CECH).map(|r| (r[z] > 0.0) as u8).collect();
            (najlepszy_sync(&b), z)
        })
        .min()?;
    if bledy > BLEDY_SYNC {
        return None;
    }
    // Pierwszy przebieg: jeden zaczep, sprzężenie z wagą zero (jak sam znak próbki).
    let mut w = vec![0.0f32; CECH_DFE];
    w[zaczep] = 1.0;
    let mut najlepszy: Option<(usize, Vec<Paczka>)> = None;
    let mut bez_poprawy = 0;
    for _ in 0..PRZEBIEGOW {
        let b = decyzje_dfe(f, &w);
        let mut trening = Vec::new();
        for p in paczki_luzne(&b) {
            dodaj_cele(&mut trening, p.poz, SYNC, p.odwr);
            for (m, &(raw, r)) in p.slowa.iter().enumerate() {
                let znane = if (raw ^ IDLE).count_ones() <= 5 {
                    Some(IDLE)
                } else {
                    r.filter(|&(_, n)| n <= 1).map(|(c, _)| c)
                };
                if let Some(c) = znane {
                    dodaj_cele(&mut trening, p.poz + 32 * (m + 1), c, p.odwr);
                }
            }
        }
        let pewne = paczki_pewne(&b);
        let wynik: usize = pewne.iter().map(|p| p.pewnych).sum();
        let wszystkie = wynik > 0 && wynik == pewne.iter().map(|p| p.slowa.len()).sum::<usize>();
        if najlepszy.as_ref().is_none_or(|(n, _)| wynik > *n) {
            najlepszy = Some((wynik, pewne));
            bez_poprawy = 0;
        } else {
            bez_poprawy += 1;
        }
        if wszystkie || bez_poprawy >= 2 || trening.len() < MIN_TRENINGU {
            break;
        }
        match naucz(&wiersze_dfe(f, &b, &trening), CECH_DFE, &trening) {
            Some(nowe) => w = nowe,
            None => break,
        }
    }
    najlepszy
}

pub fn dekoduj_predkosc(x: &[f32], fs: f32, predkosc: u32) -> Vec<Wiadomosc> {
    let fs = fs as f64;
    let co = ((fs / (PROBEK_NA_BIT * predkosc as f64)) as usize).max(1);
    let y = dolnoprzepustowy(x, fs, (2.0 * predkosc as f64).min(0.45 * fs));
    let y: Vec<f64> = y.into_iter().step_by(co).collect();
    let fs = fs / co as f64;
    let spb = fs / predkosc as f64;
    let margines = (ZASIEG_BITOW + 1) as f64 * spb;
    if (y.len() as f64) < 2.0 * margines + 64.0 * spb {
        return Vec::new();
    }
    // Linia zegara: kwadrat pochodnej przesunięty o częstotliwość bitową, sumowany w blokach
    // po ćwierć bitu. Wskaz obraca się rekurencyjnie, bez sin/cos dla każdej próbki.
    let w0 = 2.0 * PI * predkosc as f64 / fs;
    let blok = ((spb / 4.0) as usize).max(1);
    let (sk, ck) = w0.sin_cos();
    let (mut wr, mut wi) = (1.0f64, 0.0f64);
    let (mut sr, mut si) = (0.0f64, 0.0f64);
    let mut bloki = Vec::with_capacity(y.len() / blok + 1);
    for i in 0..y.len() {
        let d = if i > 0 { (y[i] - y[i - 1]).powi(2) } else { 0.0 };
        sr += d * wr;
        si += d * wi;
        if (i + 1) % blok == 0 {
            bloki.push((sr, si));
            (sr, si) = (0.0, 0.0);
        }
        (wr, wi) = (wr * ck + wi * sk, wi * ck - wr * sk);
        if i % 4096 == 4095 {
            let m = wr.hypot(wi);
            (wr, wi) = (wr / m, wi / m);
        }
    }
    let mut najlepszy: Option<(usize, Vec<Paczka>, Vec<f64>)> = None;
    for stala in STALE_ZEGARA {
        let a = (-(blok as f64) / (stala * spb)).exp();
        let t = chwile_bitow(&bloki, blok, w0, a, y.len() as f64, margines);
        if t.len() < 64 {
            continue;
        }
        let f = cechy(&y, &t, spb);
        if let Some((wynik, paczki)) = ucz_i_ramkuj(&f) {
            let wszystkie = wynik > 0 && wynik == paczki.iter().map(|p| p.slowa.len()).sum::<usize>();
            if najlepszy.as_ref().is_none_or(|(n, _, _)| wynik > *n) {
                najlepszy = Some((wynik, paczki, t));
            }
            // Każde słowo już poprawne: inna stała zegara nic nie doda.
            if wszystkie {
                break;
            }
        }
    }
    let Some((_, paczki, t)) = najlepszy else {
        return Vec::new();
    };
    zloz(&paczki, t.len())
        .into_iter()
        .filter(Surowa::wiarygodna)
        .map(|s| Wiadomosc {
            predkosc,
            ric: s.ric,
            funkcja: s.funkcja,
            dane: s.dane,
            poprawki: s.poprawki,
            bledne_slowa: s.bledne,
            probka: (t[s.bit] * co as f64).round() as usize,
            zakonczona: s.zakonczona,
        })
        .collect()
}

/// Wszystkie prędkości naraz, w kolejności nadania.
pub fn dekoduj(x: &[f32], fs: f32) -> Vec<Wiadomosc> {
    let mut w: Vec<Wiadomosc> = PREDKOSCI.iter().flat_map(|&p| dekoduj_predkosc(x, fs, p)).collect();
    w.sort_by_key(|w| w.probka);
    w
}

/// Strumień w przesuwanym oknie: każda wiadomość wychodzi raz, gdy jest już cała.
pub struct DekoderNaZywo {
    fs: f32,
    bufor: VecDeque<f32>,
    /// Numer próbki bufor[0] od startu strumienia.
    poczatek: usize,
    nowych: usize,
    krok: usize,
    ostatni_sygnal: Option<usize>,
    wyslane: Vec<(u32, u32, usize)>,
}

impl DekoderNaZywo {
    /// `krok_s`: co tyle sekund nowych próbek okno jest dekodowane od nowa.
    pub fn new(fs: f32, krok_s: f32) -> Self {
        Self {
            fs,
            bufor: VecDeque::new(),
            poczatek: 0,
            nowych: 0,
            krok: (fs * krok_s) as usize,
            ostatni_sygnal: None,
            wyslane: Vec::new(),
        }
    }

    /// Numer próbki zaraz za ostatnią przyjętą.
    pub fn koniec(&self) -> usize {
        self.poczatek + self.bufor.len()
    }

    pub fn dodaj(&mut self, probki: &[f32]) -> Vec<Wiadomosc> {
        if !probki.is_empty() {
            let rms = (probki.iter().map(|v| v * v).sum::<f32>() / probki.len() as f32).sqrt();
            if rms >= SYGNAL_RMS {
                self.ostatni_sygnal = Some(self.koniec() + probki.len());
            }
        }
        self.bufor.extend(probki.iter().copied());
        self.nowych += probki.len();
        let limit = (OKNO_S * self.fs) as usize;
        if self.bufor.len() > limit {
            let nadmiar = self.bufor.len() - limit;
            self.bufor.drain(..nadmiar);
            self.poczatek += nadmiar;
        }
        if self.nowych < self.krok {
            return Vec::new();
        }
        self.nowych = 0;
        self.dekoduj_okno(false)
    }

    /// Koniec strumienia: oddaje też wiadomości urwane.
    pub fn zakoncz(&mut self) -> Vec<Wiadomosc> {
        self.dekoduj_okno(true)
    }

    fn dekoduj_okno(&mut self, wszystkie: bool) -> Vec<Wiadomosc> {
        // Sama cisza w oknie: nie ma czego szukać.
        if !self.ostatni_sygnal.is_some_and(|k| k > self.poczatek) {
            return Vec::new();
        }
        let poczatek = self.poczatek;
        let mut nowe = Vec::new();
        for mut w in dekoduj(self.bufor.make_contiguous(), self.fs) {
            if !w.zakonczona && !wszystkie {
                continue;
            }
            w.probka += poczatek;
            // Ta sama wiadomość w kolejnych oknach: ten sam adres w prawie tym samym miejscu.
            let tol = (self.fs / w.predkosc as f32 * 8.0) as usize;
            if self.wyslane.iter().any(|&(p, r, s)| p == w.predkosc && r == w.ric && s.abs_diff(w.probka) <= tol) {
                continue;
            }
            self.wyslane.push((w.predkosc, w.ric, w.probka));
            nowe.push(w);
        }
        self.wyslane.retain(|&(_, _, s)| s + self.bufor.len() >= poczatek);
        nowe
    }
}

/// Cały plik przez okno, żeby długie nagranie nie zajęło pamięci naraz.
pub fn dekoduj_plik(x: &[f32], fs: f32) -> Vec<Wiadomosc> {
    let mut d = DekoderNaZywo::new(fs, 3.0);
    let mut wyj = Vec::new();
    for k in x.chunks((fs * 0.25) as usize) {
        wyj.extend(d.dodaj(k));
    }
    wyj.extend(d.zakoncz());
    wyj
}

#[cfg(test)]
mod testy {
    use super::*;

    fn bch_koduj(d21: u32) -> u32 {
        let r = syndrom(d21 << 10);
        let cw = ((d21 << 10) | r) << 1;
        cw | (cw.count_ones() & 1)
    }

    fn slowa_tekstu(tekst: &str) -> Vec<u32> {
        let mut bity: Vec<u32> = Vec::new();
        for c in tekst.bytes().chain([4u8]) {
            bity.extend((0..7).map(|j| (c as u32 >> j) & 1));
        }
        while bity.len() % 20 != 0 {
            bity.push(0);
        }
        bity.chunks(20).map(|z| bch_koduj((1 << 20) | z.iter().fold(0, |a, &b| (a << 1) | b))).collect()
    }

    fn koduj(wiad: &[(u32, u8, &str)]) -> Vec<u8> {
        let mut slowa = Vec::new();
        for &(ric, funkcja, tekst) in wiad {
            while (slowa.len() % SLOW_W_PACZCE) / 2 != (ric & 7) as usize {
                slowa.push(IDLE);
            }
            slowa.push(bch_koduj(((ric >> 3) << 2) | funkcja as u32));
            slowa.extend(slowa_tekstu(tekst));
        }
        slowa.push(IDLE);
        while slowa.len() % SLOW_W_PACZCE != 0 {
            slowa.push(IDLE);
        }
        let mut bity: Vec<u8> = (0..576).map(|i| (i % 2 == 0) as u8).collect();
        for paczka in slowa.chunks(SLOW_W_PACZCE) {
            for &w in std::iter::once(&SYNC).chain(paczka) {
                bity.extend((0..32).map(|i| ((w >> (31 - i)) & 1) as u8));
            }
        }
        bity
    }

    struct Los(u32);
    impl Los {
        fn gauss(&mut self) -> f32 {
            let mut s = 0.0;
            for _ in 0..12 {
                self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
                s += (self.0 >> 8) as f32 / (1u32 << 24) as f32;
            }
            s - 6.0
        }
    }

    fn jednobiegunowy(x: &mut [f32], fs: f32, fc: f32, gorny: bool) {
        let a = (-2.0 * std::f32::consts::PI * fc / fs).exp();
        let mut lp = 0.0f32;
        for v in x.iter_mut() {
            lp = a * lp + (1.0 - a) * *v;
            *v = if gorny { *v - lp } else { lp };
        }
    }

    /// NRZ przez tor jak w radiu: filtr nadajnika, deemfaza, górnoprzepustowy 4. rzędu, szum.
    fn nadaj(bity: &[u8], predkosc: u32, fs: f32, odwr: bool, deemfaza: bool, los: &mut Los) -> Vec<f32> {
        let spb = fs / predkosc as f32 * (1.0 + 30e-6);
        let cisza = (0.4 * fs) as usize;
        let dl = (bity.len() as f32 * spb) as usize;
        let mut x = vec![0.0f32; cisza + dl + cisza];
        for (i, v) in x[cisza..cisza + dl].iter_mut().enumerate() {
            let b = bity[((i as f32 / spb) as usize).min(bity.len() - 1)];
            *v = if (b == 1) != odwr { 1.0 } else { -1.0 };
        }
        jednobiegunowy(&mut x, fs, predkosc as f32 * 0.8, false);
        jednobiegunowy(&mut x, fs, predkosc as f32 * 0.8, false);
        if deemfaza {
            jednobiegunowy(&mut x, fs, 300.0, false);
        }
        for _ in 0..4 {
            jednobiegunowy(&mut x, fs, 250.0, true);
        }
        let rms = (x.iter().map(|v| v * v).sum::<f32>() / dl as f32).sqrt();
        for (i, v) in x.iter_mut().enumerate() {
            let nadaje = i + (0.05 * fs) as usize >= cisza && i < cisza + dl + (0.05 * fs) as usize;
            *v = if nadaje { 0.2 * (*v / rms + 0.1 * los.gauss()) } else { 0.0 };
        }
        x
    }

    const WIAD: [(u32, u8, &str); 2] = [(1234567, 3, "TEST POCSAG 123"), (123452, 2, "TEST DRUGIEJ RAMKI")];

    fn odczytane(w: &[Wiadomosc]) -> Vec<(u32, u8, String)> {
        w.iter().map(|w| (w.ric, w.funkcja, w.tekst())).collect()
    }

    fn oczekiwane() -> Vec<(u32, u8, String)> {
        WIAD.iter().map(|&(r, f, t)| (r, f, t.to_string())).collect()
    }

    #[test]
    fn bch_poprawia_dwa_bity() {
        let c = bch_koduj((123452 >> 3) << 2 | 2);
        assert_eq!(popraw(c), Some((c, 0)));
        for i in 0..32 {
            assert_eq!(popraw(c ^ (1 << i)), Some((c, 1)), "bit {i}");
            for j in i + 1..32 {
                assert_eq!(popraw(c ^ (1 << i) ^ (1 << j)), Some((c, 2)), "bity {i} {j}");
            }
        }
        assert_eq!(rozpoznaj(0), None, "cisza to nie adres");
    }

    #[test]
    fn tekst_i_cyfry() {
        let dane = |s| slowa_tekstu(s).iter().map(|w| (w >> 11) & 0xFFFFF).collect();
        let w = Wiadomosc {
            predkosc: 512,
            ric: 1,
            funkcja: 0,
            dane: dane("TEST ODBIORU"),
            poprawki: 0,
            bledne_slowa: 0,
            probka: 0,
            zakonczona: true,
        };
        assert_eq!(w.tekst(), "TEST ODBIORU");
        assert!(w.wyglada_na_tekst());
        // Cyfry 1,2,3 i wypełnienie spacjami (0xC).
        let w = Wiadomosc { dane: vec![0b1000_0100_1100_0011_0011], ..w };
        assert_eq!(w.cyfry(), "123");
        assert!(!w.wyglada_na_tekst());
    }

    #[test]
    fn trzy_predkosci_przez_tor_radia() {
        let mut los = Los(7);
        let bity = koduj(&WIAD);
        for predkosc in PREDKOSCI {
            for odwr in [false, true] {
                for deemfaza in [false, true] {
                    // 2400 bit/s po deemfazie gubił kiedyś koniec tekstu (długie ciągi zer giną
                    // w filtrze górnoprzepustowym); od korektora ze sprzężeniem przechodzi.
                    let x = nadaj(&bity, predkosc, 48000.0, odwr, deemfaza, &mut los);
                    let w = dekoduj(&x, 48000.0);
                    assert_eq!(odczytane(&w), oczekiwane(), "{predkosc} odwr={odwr} deemfaza={deemfaza}");
                    assert!(w.iter().all(|w| w.predkosc == predkosc && w.zakonczona));
                }
            }
        }
    }

    #[test]
    fn szum_i_ton_bez_wiadomosci() {
        let fs = 48000.0;
        let mut los = Los(99);
        let szum: Vec<f32> = (0..(fs * 10.0) as usize).map(|_| 0.1 * los.gauss()).collect();
        assert!(dekoduj(&szum, fs).is_empty());
        // Kwadrat 256 Hz wygląda jak preambuła 512 bit/s, ale bez syncu to nie wiadomość.
        let ton: Vec<f32> = (0..(fs * 5.0) as usize).map(|i| if (i as f32 / fs * 512.0) as u32 % 2 == 0 { 0.2 } else { -0.2 }).collect();
        assert!(dekoduj(&ton, fs).is_empty());
    }

    #[test]
    fn na_zywo_kazda_wiadomosc_raz() {
        let fs = 16000.0;
        let mut los = Los(3);
        let mut x = vec![0.0f32; (fs * 2.0) as usize];
        x.extend(nadaj(&koduj(&WIAD[..1]), 1200, fs, false, true, &mut los));
        x.extend(vec![0.0; (fs * 3.0) as usize]);
        x.extend(nadaj(&koduj(&WIAD[1..]), 512, fs, true, true, &mut los));
        x.extend(vec![0.0; (fs * 3.0) as usize]);
        let mut d = DekoderNaZywo::new(fs, 1.0);
        let mut w = Vec::new();
        for k in x.chunks((fs * 0.1) as usize) {
            w.extend(d.dodaj(k));
        }
        assert_eq!(odczytane(&w), oczekiwane());
        assert!(d.zakoncz().is_empty(), "nic urwanego na końcu");
        assert_eq!(odczytane(&dekoduj_plik(&x, fs)), oczekiwane());
    }

    /// Karta albo kabel wirtualny pod obciążeniem gubi czasem cały pakiet (10 ms): dekoder może
    /// wtedy zgubić wiadomość, ale nie może wymyślać adresów z przesuniętych słów. 10 ms (5,25 bitu
    /// przy 512 bit/s) wycinane po kolei w różnych miejscach nadania.
    fn przeskoki(predkosc: u32) -> (usize, usize, Vec<u32>) {
        let fs = 44100.0f32;
        let bity = koduj(&[(1234567, 3, "Test petli 512 zazolc"), (123454, 2, "TEST DRUGIEJ RAMKI")]);
        let spb = fs / predkosc as f32;
        let dl = (bity.len() as f32 * spb) as usize;
        let mut x = vec![0.0f32; (0.4 * fs) as usize];
        x.extend((0..dl).map(|i| if bity[((i as f32 / spb) as usize).min(bity.len() - 1)] == 1 { -0.5 } else { 0.5 }));
        x.extend(vec![0.0; fs as usize]);
        jednobiegunowy(&mut x, fs, predkosc as f32 * 0.8, false);
        jednobiegunowy(&mut x, fs, predkosc as f32 * 0.8, false);
        let (mut proby, mut dobre, mut falszywe) = (0, 0, Vec::new());
        let mut t = 0.3;
        while t < bity.len() as f32 / predkosc as f32 {
            let start = ((0.4 + t) * fs) as usize;
            let mut y = x.clone();
            y.drain(start..start + (5.25 * spb) as usize);
            let w = dekoduj(&y, fs);
            proby += 1;
            dobre += w.iter().filter(|w| w.ric == 123454 && w.tekst() == "TEST DRUGIEJ RAMKI").count();
            falszywe.extend(w.iter().map(|w| w.ric).filter(|r| ![1234567, 123454].contains(r)));
            t += 0.05;
        }
        (proby, dobre, falszywe)
    }

    /// Słowa kodowe w strefie przesuniętego idle są zawsze brane za idle: ma ich być znikomo mało.
    #[test]
    fn slowa_kodowe_poza_strefa_idle() {
        let wzory = idle_przesuniete();
        let n = (0..1u32 << 21)
            .map(bch_koduj)
            .filter(|&c| c != IDLE && wzory.iter().any(|&w| (c ^ w).count_ones() <= 3))
            .count();
        assert_eq!(n, 78);
        assert_eq!(rozpoznaj(0), None, "cisza to nie adres");
    }

    /// Przed poprawką (łańcuch paczek, odstęp paczek po przeskoku, idle przesunięte, paczka
    /// z przeskokiem, adres przed przesuniętym idle): 512 bit/s 490 fałszywych RIC na 81 prób
    /// i druga wiadomość cała 49 razy, 1200 bit/s 210 na 31 i 17. Po niej: 1 i 54, 0 i 19.
    /// Po końcu nadania radio oddaje szum (otwarta blokada szumów). Dekoder doklejał z niego paczki
    /// do łańcucha i robił fałszywe adresy (np. 04.11 trzy obce RIC sekundę po wywołaniu).
    #[test]
    fn szum_po_nadaniu_bez_falszywych() {
        let fs = 44100.0f32;
        for predkosc in [512u32, 1200] {
            let bity = koduj(&[(1000100, 3, "101: TAK")]);
            let spb = fs / predkosc as f32;
            for ziarno in 0..10u32 {
                let mut los = Los(12345u32.wrapping_add(ziarno.wrapping_mul(2654435761)));
                let mut x = vec![0.0f32; (0.5 * fs) as usize];
                let dl = (bity.len() as f32 * spb) as usize;
                x.extend((0..dl).map(|i| if bity[((i as f32 / spb) as usize).min(bity.len() - 1)] == 1 { -0.5 } else { 0.5 }));
                jednobiegunowy(&mut x, fs, predkosc as f32 * 0.8, false);
                x.extend((0..(2.0 * fs) as usize).map(|_| 0.25 * los.gauss()));
                x.extend(vec![0.0f32; (2.0 * fs) as usize]);
                let w = dekoduj_plik(&x, fs);
                assert!(w.iter().any(|w| w.ric == 1000100 && w.tekst() == "101: TAK"), "{predkosc} ziarno {ziarno}: brak wiadomości");
                let obce: Vec<u32> = w.iter().map(|w| w.ric).filter(|&r| r != 1000100).collect();
                assert!(obce.is_empty(), "{predkosc} ziarno {ziarno}: fałszywe {obce:?}");
            }
        }
    }

    #[test]
    fn przeskok_bez_falszywych_adresow() {
        for predkosc in [512, 1200] {
            let (p, d, f) = przeskoki(predkosc);
            println!("{predkosc} bit/s: prób {p}, wiadomość 2 cała {d}, fałszywe RIC {}", f.len());
            assert!(f.len() * 10 <= p, "{predkosc}: {} fałszywych na {p} prób: {f:?}", f.len());
            assert!(d * 10 >= p * 6, "{predkosc}: wiadomość 2 cała tylko {d} na {p}");
        }
    }
}
