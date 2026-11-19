//! Inwersja widma: f -> nośna - f. Ta sama operacja szyfruje i deszyfruje.
//!
//! Tor jak w deinvert (Oona Räisänen, MIT): filtr, mieszanie, wycięcie resztek. Tu filtr jest
//! zespolony, więc na wyjściu nie ma produktu sumy i działa to także przy 8 kHz,
//! gdzie zwykłe mnożenie przez cos dawałoby aliasy w paśmie mowy.

use std::f32::consts::PI;

/// Tony scramblera Quansheng UV-K5 (BK4819, rejestr 0x71 = 0x68DC + n * 1032).
pub const TONY_QUANSHENG: [f32; 10] = [
    2600.0, 2700.0, 2800.0, 2900.0, 3000.0, 3100.0, 3200.0, 3300.0, 3400.0, 3500.0,
];

/// Tony do ręcznego wyboru w oknie: lista Quansheng plus pośrednie co 50 Hz, np. 2950 obcej stacji.
/// Automat ich nie używa: przyciąganie do gęstszej listy psułoby nośne takie jak 2632 Hz.
pub const TONY_RECZNE: [f32; 19] = [
    2600.0, 2650.0, 2700.0, 2750.0, 2800.0, 2850.0, 2900.0, 2950.0, 3000.0, 3050.0, 3100.0, 3150.0, 3200.0,
    3250.0, 3300.0, 3350.0, 3400.0, 3450.0, 3500.0,
];

/// Dolna granica wejścia: wycina CTCSS (do 254 Hz), który po inwersji byłby piskiem.
const WEJSCIE_OD_HZ: f32 = 300.0;
/// Odstęp góry wejścia od nośnej: po inwersji daje dolną granicę mowy.
const ODSTEP_OD_NOSNEJ_HZ: f32 = 150.0;
const DLUGOSC_FILTRA_S: f32 = 0.03;

pub struct Inwerter {
    fs: f32,
    h_re: Vec<f32>,
    h_im: Vec<f32>,
    /// Bufor podwójny: każde okno filtra jest ciągłym wycinkiem.
    bufor: Vec<f32>,
    poz: usize,
    faza: f32,
    krok: f32,
}

impl Inwerter {
    pub fn new(fs: f32, nosna: f32) -> Self {
        let n = ((fs * DLUGOSC_FILTRA_S) as usize) | 1;
        let mut s = Self {
            fs,
            h_re: vec![0.0; n],
            h_im: vec![0.0; n],
            bufor: vec![0.0; 2 * n],
            poz: 0,
            faza: 0.0,
            krok: 0.0,
        };
        s.ustaw_nosna(nosna);
        s
    }

    pub fn ustaw_nosna(&mut self, nosna: f32) {
        let hi = (nosna - ODSTEP_OD_NOSNEJ_HZ).min(self.fs / 2.0 - 100.0);
        let lo = WEJSCIE_OD_HZ;
        let srodek = (lo + hi) / 2.0;
        let pol = (hi - lo) / 2.0;
        let n = self.h_re.len();
        let m = (n - 1) as f32 / 2.0;
        for i in 0..n {
            let t = i as f32 - m;
            let x = 2.0 * pol / self.fs * t;
            let sinc = if t == 0.0 { 1.0 } else { (PI * x).sin() / (PI * x) };
            let okno = 0.54 + 0.46 * (2.0 * PI * t / (n - 1) as f32).cos();
            let a = 2.0 * pol / self.fs * sinc * okno;
            let w = 2.0 * PI * srodek / self.fs * t;
            // Okno w przetworz() idzie od najstarszej próbki, stąd h(-t).
            self.h_re[i] = a * w.cos();
            self.h_im[i] = -a * w.sin();
        }
        self.krok = 2.0 * PI * nosna / self.fs;
    }

    #[inline]
    pub fn przetworz(&mut self, x: f32) -> f32 {
        let n = self.h_re.len();
        self.bufor[self.poz] = x;
        self.bufor[self.poz + n] = x;
        self.poz = (self.poz + 1) % n;
        // Po wpisie okno [poz, poz+n) idzie od najstarszej do najnowszej próbki.
        let okno = &self.bufor[self.poz..self.poz + n];
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for ((p, a), b) in okno.iter().zip(&self.h_re).zip(&self.h_im) {
            re += p * a;
            im += p * b;
        }
        self.faza += self.krok;
        if self.faza > 2.0 * PI {
            self.faza -= 2.0 * PI;
        }
        // 2*Re(conj(z) * e^(j*faza)): z sygnału analitycznego f zostaje tylko nośna - f.
        2.0 * (re * self.faza.cos() + im * self.faza.sin())
    }
}

pub fn odwroc(probki: &[f32], fs: f32, nosna: f32) -> Vec<f32> {
    let mut inw = Inwerter::new(fs, nosna);
    let mut wyj: Vec<f32> = probki.iter().map(|&x| inw.przetworz(x)).collect();
    // Filtr opóźnia o połowę długości, a plik ma się pokrywać z wejściem.
    let opoznienie = ((fs * DLUGOSC_FILTRA_S) as usize | 1) / 2;
    wyj.drain(..opoznienie.min(wyj.len()));
    wyj
}

#[cfg(test)]
mod testy {
    use super::*;

    fn ton(f: f32, fs: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * f * i as f32 / fs).sin()).collect()
    }

    fn poziom(x: &[f32], f: f32, fs: f32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, v) in x.iter().enumerate() {
            let w = 2.0 * PI * f * i as f32 / fs;
            re += v * w.cos();
            im += v * w.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f32
    }

    #[test]
    fn ton_trafia_w_lustro_bez_produktu_sumy() {
        for fs in [8000.0, 44100.0, 48000.0] {
            let wej = ton(1000.0, fs, fs as usize);
            let wyj = odwroc(&wej, fs, 3000.0);
            let srodek = &wyj[wyj.len() / 4..3 * wyj.len() / 4];
            assert!((poziom(srodek, 2000.0, fs) - 1.0).abs() < 0.02, "fs={fs}");
            assert!(poziom(srodek, 1000.0, fs) < 0.01, "fs={fs}");
            let suma = 4000.0f32;
            let alias = if suma > fs / 2.0 { fs - suma } else { suma };
            assert!(poziom(srodek, alias, fs) < 0.01, "fs={fs}");
        }
    }

    /// 4096 Hz to więcej niż połowa 8 kHz: oscylator się zawija, a lustro ma i tak wyjść dobrze.
    #[test]
    fn nosna_4096_takze_przy_8khz() {
        for fs in [8000.0, 48000.0] {
            let wej = ton(1000.0, fs, fs as usize);
            let wyj = odwroc(&wej, fs, 4096.0);
            let srodek = &wyj[wyj.len() / 4..3 * wyj.len() / 4];
            assert!((poziom(srodek, 3096.0, fs) - 1.0).abs() < 0.02, "fs={fs}");
            assert!(poziom(srodek, 1000.0, fs) < 0.01, "fs={fs}");
            let suma = 5096.0f32;
            let alias = if suma > fs / 2.0 { fs - suma } else { suma };
            assert!(poziom(srodek, alias, fs) < 0.01, "fs={fs}");
        }
    }

    #[test]
    fn ctcss_nie_przechodzi() {
        let fs = 48000.0;
        let wej = ton(250.3, fs, fs as usize);
        let wyj = odwroc(&wej, fs, 3000.0);
        let srodek = &wyj[wyj.len() / 4..3 * wyj.len() / 4];
        assert!(poziom(srodek, 3000.0 - 250.3, fs) < 0.05);
    }
}
