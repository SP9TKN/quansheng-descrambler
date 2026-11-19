//! Wykrywanie tonu scramblera z samego sygnału.
//!
//! Harmoniczne głosu leżą na k*F0, a po inwersji na nosna - k*F0. W obu przypadkach tworzą
//! grzebień o odstępie F0, różni się tylko punkt zaczepienia: 0 albo nosna (modulo F0).
//! Faza grzebienia w ramce podaje ten punkt, a zmienność F0 między ramkami usuwa
//! wieloznaczność modulo. Cecha podpatrzona u Oony Räisänen (deinvert), detekcja własna.

use crate::dsp::TONY_QUANSHENG;
use rustfft::{FftPlanner, num_complex::Complex32};
use std::f32::consts::PI;

const FS_ANALIZY_MIN: f32 = 8000.0;
const RAMKA_S: f32 = 0.064;
const NFFT: usize = 2048;
const F0_OD: f32 = 75.0;
const F0_DO: f32 = 320.0;
const F0_KROK_WZGL: f32 = 0.01;
const PASMO_OD_HZ: f32 = 300.0;
const PASMO_DO_HZ: f32 = 3700.0;
/// Promień wygładzania przy spłaszczaniu widma: większy od odstępu harmonicznych.
const WYGLADZANIE_HZ: f32 = 250.0;
const CISZA_RMS: f32 = 0.002;
/// Ramki co 16 ms: przy kroku 32 ms przesunięcie początku nagrania o ułamek ramki potrafiło
/// zamienić remis nośnej z jej aliasem (o jedno F0) w wyraźną wygraną jednej z nich.
const KROKOW_NA_RAMKE: usize = 4;
/// Progi w ramkach liczone przy kroku 32 ms, przeliczane na obecny krok.
const RAMEK_NA_32MS: usize = KROKOW_NA_RAMKE / 2;
const MAX_RAMEK: usize = 300 * RAMEK_NA_32MS;
/// Szerokość listka punktacji: bliżsi kandydaci to ten sam szczyt, nie rywale.
const LISTEK_HZ: f32 = 40.0;
/// Waga obwiedni: tylko rozstrzyga remisy, gdy F0 jest podzielnikiem różnicy kandydatów.
const WAGA_OBWIEDNI: f32 = 0.25;
const PROG_PUNKTOW: f32 = 0.25;
const PROG_PRZEWAGI: f32 = 0.08;
/// Poniżej tego (ok. 1 s mowy) F0 zmienia się za mało, by rozróżnić kandydatów.
const MIN_RAMEK: usize = 30 * RAMEK_NA_32MS;
/// Na żywo dla listy Quansheng i braku inwersji: 11 możliwości zamiast całego skanu,
/// a pomyłkę łapie wymóg powtórzenia. Progi ze strojenia na mowie i nagraniu z IC-7100.
const MIN_RAMEK_WSTEPNIE: usize = 8 * RAMEK_NA_32MS;
const PROG_PRZEWAGI_LISTY: f32 = 0.08;
/// Na kanałach użytkownika najczęściej słychać 2800-3000 Hz. Pośrednie tony z tego pasma
/// dostają szybką ścieżkę jak lista Quansheng; 2800, 2900 i 3000 już ją mają.
const TONY_CZESTE: [f32; 2] = [2850.0, 2950.0];
/// Szum daje około 0,38 niezależnie od poziomu, głos zwykle powyżej 0,5.
const PROG_SILY: f32 = 0.46;
/// Więcej niż tonów w beepie: 800-600-700-800 Hz daje w ramce najwyżej trzy.
const MIN_SZCZYTOW: usize = 4;
const ODSTEP_SZCZYTOW_HZ: f32 = 50.0;
/// Zakres nośnych: od najniższych w programowalnych scramblerach (ok. 2100) do 4096 z zapasem.
pub const SKAN_OD_HZ: f32 = 2000.0;
pub const SKAN_DO_HZ: f32 = 4200.0;
const SKAN_KROK_HZ: f32 = 5.0;
/// Tak blisko tonu z listy to już ten ton: szczyt ma rozrzut rzędu kilku Hz.
const PRZYCIAGANIE_HZ: f32 = 15.0;
/// Okno szukania aliasu wokół zwycięzcy, w krotnościach mediany F0.
const ALIAS_OD: f32 = 0.75;
const ALIAS_DO: f32 = 1.3;

#[derive(Clone, Debug)]
pub struct Wynik {
    /// `None` = sygnał bez inwersji.
    pub ton: Option<f32>,
    pub punkty: f32,
    /// Punkty zwycięzcy minus najlepszy kandydat spoza jego listka.
    pub przewaga: f32,
    pub ramek: usize,
    /// Zwycięzca odstaje na tyle, by mu ufać. Fałsz: inny scrambler albo za mało mowy.
    pub pewny: bool,
    /// Punkty tonów Quansheng (do podglądu), w kolejności `TONY_QUANSHENG`.
    pub punkty_tonow: Vec<f32>,
    /// Przewaga nad pozostałymi tonami z listy i brakiem inwersji; 0, gdy zwycięzca jest spoza listy.
    pub przewaga_listy: f32,
    /// Wystarcza do decyzji na żywo, jeśli powtórzy się w kolejnej analizie.
    pub wstepny: bool,
    /// Średnia siła grzebienia w ramce: niska dla szumu, wysoka dla głosu.
    pub sila: f32,
    /// Pełny skan: brak inwersji, potem kandydaci od `SKAN_OD_HZ` co `SKAN_KROK_HZ`.
    pub skan: Vec<(Option<f32>, f32)>,
    /// Mediana F0 ramek ważona siłą grzebienia.
    pub f0_mediana: f32,
    /// Przewaga nad najlepszym kandydatem odległym o około jedno F0 (alias przy stałym F0).
    pub przewaga_aliasu: f32,
}

impl Wynik {
    /// Punkty nośnej ze skanu, bez dodatkowych obliczeń; najbliższy kandydat siatki.
    pub fn punkty_przy(&self, nosna: f32) -> f32 {
        let i = ((nosna - SKAN_OD_HZ) / SKAN_KROK_HZ).round().max(0.0) as usize;
        self.skan.get(i + 1).map_or(0.0, |p| p.1)
    }

    /// Najlepsze szczyty skanu, malejąco, oddalone od siebie o więcej niż listek.
    /// Przy remisie z aliasem pokazuje obie możliwości.
    pub fn szczyty(&self, ile: usize) -> Vec<(Option<f32>, f32)> {
        let s = &self.skan[1..];
        let mut lokalne: Vec<(Option<f32>, f32)> = (1..s.len().saturating_sub(1))
            .filter(|&i| s[i].1 > s[i - 1].1 && s[i].1 >= s[i + 1].1)
            .map(|i| s[i])
            .chain(std::iter::once(self.skan[0]))
            .collect();
        lokalne.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut wynik: Vec<(Option<f32>, f32)> = Vec::new();
        for k in lokalne {
            let daleko = wynik.iter().all(|w| match (w.0, k.0) {
                (Some(a), Some(b)) => (a - b).abs() > LISTEK_HZ,
                (None, None) => false,
                _ => true,
            });
            if daleko {
                wynik.push(k);
            }
            if wynik.len() == ile {
                break;
            }
        }
        wynik
    }
}

/// Grzebień jednej ramki: zespolony współczynnik przy najlepszym F0.
struct Grzebien {
    f0: f32,
    re: f32,
    im: f32,
}

impl Grzebien {
    fn sila(&self) -> f32 {
        self.re.hypot(self.im)
    }

    fn zgodnosc(&self, zaczep: f32) -> f32 {
        let w = 2.0 * PI * zaczep / self.f0;
        self.re * w.cos() - self.im * w.sin()
    }
}

fn decymuj(x: &[f32], fs: f32) -> (Vec<f32>, f32) {
    let m = (fs / FS_ANALIZY_MIN).floor().max(1.0) as usize;
    if m == 1 {
        return (x.to_vec(), fs);
    }
    let fs_a = fs / m as f32;
    let n = 8 * m + 1;
    let c = (n - 1) as f32 / 2.0;
    let fc = 0.47 * fs_a / fs;
    let h: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 - c;
            let s = if t == 0.0 { 1.0 } else { (2.0 * PI * fc * t).sin() / (2.0 * PI * fc * t) };
            2.0 * fc * s * (0.54 + 0.46 * (2.0 * PI * t / (n - 1) as f32).cos())
        })
        .collect();
    let wyj = x
        .windows(n)
        .step_by(m)
        .map(|w| w.iter().zip(&h).map(|(a, b)| a * b).sum())
        .collect();
    (wyj, fs_a)
}

/// Liczy szczyty widma: co najmniej 1% mocy najsilniejszego (ponad listki boczne okna)
/// i oddalone od silniejszych.
fn liczba_szczytow(widmo: &[Complex32], odstep: usize) -> usize {
    let moc: Vec<f32> = widmo.iter().map(|c| c.norm_sqr()).collect();
    let szczyt = moc.iter().copied().fold(0.0, f32::max);
    let mut kandydaci: Vec<usize> = (1..moc.len().saturating_sub(1))
        .filter(|&i| moc[i] >= 0.01 * szczyt && moc[i] >= moc[i - 1] && moc[i] > moc[i + 1])
        .collect();
    kandydaci.sort_by(|&a, &b| moc[b].total_cmp(&moc[a]));
    let mut wybrane: Vec<usize> = Vec::new();
    for k in kandydaci {
        if wybrane.iter().all(|&w| w.abs_diff(k) >= odstep) {
            wybrane.push(k);
        }
    }
    wybrane.len()
}

struct Widma {
    /// Spłaszczone ramki bez składowych stałych w czasie (CTCSS, tony), o jednostkowej mocy.
    ramki: Vec<Vec<f32>>,
    /// Średnia moc w tych samych binach, do oceny obwiedni.
    moc: Vec<f32>,
    bin_od: usize,
}

fn widma(x: &[f32], fs: f32) -> Widma {
    let dl = (fs * RAMKA_S) as usize;
    let na_bin = NFFT as f32 / fs;
    let bin_od = (PASMO_OD_HZ * na_bin) as usize;
    let bin_do = (PASMO_DO_HZ.min(fs / 2.0 - 50.0) * na_bin) as usize;
    let dlugosc = bin_do - bin_od;
    let mut moc = vec![0.0f32; dlugosc];
    if x.len() < dl {
        return Widma { ramki: Vec::new(), moc, bin_od };
    }
    let okno: Vec<f32> = (0..dl)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (dl - 1) as f32).cos())
        .collect();
    let fft = FftPlanner::<f32>::new().plan_fft_forward(NFFT);
    let mut buf = vec![Complex32::new(0.0, 0.0); NFFT];
    let transformuj = |p: usize, buf: &mut [Complex32]| {
        buf.fill(Complex32::new(0.0, 0.0));
        for i in 0..dl {
            buf[i].re = x[p + i] * okno[i];
        }
        fft.process(buf);
    };
    // Energia tylko w paśmie analizy: pauza z samym CTCSS nie może udawać mowy.
    let skala = 2.0 / (NFFT as f32 * okno.iter().map(|w| w * w).sum::<f32>());
    let mut ramki: Vec<(f32, usize)> = (0..=(x.len() - dl))
        .step_by(dl / KROKOW_NA_RAMKE)
        .map(|p| {
            transformuj(p, &mut buf);
            let e: f32 = buf[bin_od..bin_do].iter().map(|c| c.norm_sqr()).sum();
            // Ramka bez kilku harmonicznych nie jest głosem. Beep z tonów co 100 Hz pasowałby
            // do grzebienia zaczepionego w każdym tonie Quansheng.
            let glos = liczba_szczytow(&buf[bin_od..bin_do], (ODSTEP_SZCZYTOW_HZ * na_bin) as usize) >= MIN_SZCZYTOW;
            (if glos { (e * skala).sqrt() } else { 0.0 }, p)
        })
        .collect();
    let szczyt = ramki.iter().map(|r| r.0).fold(0.0, f32::max);
    let prog = (szczyt * 0.1).max(CISZA_RMS);
    ramki.retain(|r| r.0 >= prog);
    ramki.sort_by(|a, b| b.0.total_cmp(&a.0));
    ramki.truncate(MAX_RAMEK);

    let promien = (WYGLADZANIE_HZ * na_bin) as usize;
    let mut wyj: Vec<Vec<f32>> = ramki
        .iter()
        .map(|&(_, p)| {
            transformuj(p, &mut buf);
            let e_ramki: f32 = buf[bin_od..bin_do].iter().map(|c| c.norm_sqr()).sum();
            if e_ramki > 0.0 {
                for (s, c) in moc.iter_mut().zip(&buf[bin_od..bin_do]) {
                    *s += c.norm_sqr() / e_ramki;
                }
            }
            let m: Vec<f32> = buf[..NFFT / 2].iter().map(|c| c.norm().sqrt()).collect();
            // Suma krocząca po pamięci kumulacyjnej: odjęcie obwiedni formantów.
            let mut kum = vec![0.0f32; m.len() + 1];
            for i in 0..m.len() {
                kum[i + 1] = kum[i] + m[i];
            }
            (bin_od..bin_do)
                .map(|i| {
                    let a = i.saturating_sub(promien);
                    let b = (i + promien + 1).min(m.len());
                    m[i] - (kum[b] - kum[a]) / (b - a) as f32
                })
                .collect::<Vec<f32>>()
        })
        .collect();

    let mut srednia = vec![0.0f32; dlugosc];
    for r in &wyj {
        for (s, v) in srednia.iter_mut().zip(r) {
            *s += v / wyj.len() as f32;
        }
    }
    for r in &mut wyj {
        let mut e = 0.0;
        for (v, s) in r.iter_mut().zip(&srednia) {
            *v -= s;
            e += *v * *v;
        }
        let e = (e / dlugosc as f32).sqrt();
        if e > 0.0 {
            r.iter_mut().for_each(|v| *v /= e);
        }
    }
    let suma: f32 = moc.iter().sum();
    if suma > 0.0 {
        moc.iter_mut().for_each(|v| *v /= suma);
    }
    Widma { ramki: wyj, moc, bin_od }
}

/// Czy średnie widmo po odwróceniu wygląda jak mowa: najwięcej mocy w 300-900 Hz i nic
/// poniżej zera, czyli powyżej samej nośnej.
fn obwiednia(w: &Widma, df: f32, nosna: Option<f32>) -> f32 {
    let suma_w = |od: f32, dos: f32| -> f32 {
        let a = ((od / df) as usize).saturating_sub(w.bin_od).min(w.moc.len());
        let b = ((dos / df) as usize).saturating_sub(w.bin_od).min(w.moc.len());
        w.moc[a..b.max(a)].iter().sum()
    };
    match nosna {
        Some(fc) => suma_w(fc - 900.0, fc - 300.0) - suma_w(fc - 100.0, f32::MAX / 2.0),
        None => suma_w(300.0, 900.0),
    }
}

/// Współczynnik grzebienia o odstępie `f0` liczony rekurencyjnie po binach.
fn wspolczynnik(s: &[f32], bin_od: usize, df: f32, f0: f32) -> (f32, f32) {
    let krok = 2.0 * PI * df / f0;
    let start = krok * bin_od as f32;
    let (mut c, mut d) = (start.cos(), -start.sin());
    let (kc, kd) = (krok.cos(), -krok.sin());
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for &v in s {
        re += v * c;
        im += v * d;
        let n = c * kc - d * kd;
        d = c * kd + d * kc;
        c = n;
    }
    let k = 2.0 / s.len() as f32;
    (re * k, im * k)
}

fn grzebien_ramki(s: &[f32], bin_od: usize, df: f32, siatka: &[f32]) -> Grzebien {
    let mut naj = (0.0f32, siatka[0]);
    for &f0 in siatka {
        let (re, im) = wspolczynnik(s, bin_od, df, f0);
        let sila = re * re + im * im;
        if sila > naj.0 {
            naj = (sila, f0);
        }
    }
    // Dokładne F0: błąd 1 Hz przy ekstrapolacji do nośnej przesuwa zaczep o kilkanaście Hz.
    let mut f_naj = naj.1;
    let mut sila_naj = naj.0;
    for i in -8..=8 {
        let f0 = naj.1 * (1.0 + i as f32 * F0_KROK_WZGL / 8.0);
        let (re, im) = wspolczynnik(s, bin_od, df, f0);
        let sila = re * re + im * im;
        if sila > sila_naj {
            sila_naj = sila;
            f_naj = f0;
        }
    }
    let (re, im) = wspolczynnik(s, bin_od, df, f_naj);
    Grzebien { f0: f_naj, re, im }
}

/// Zwraca `None`, gdy w próbkach nie ma nic, co przypominałoby sygnał.
pub fn wykryj(probki: &[f32], fs: f32) -> Option<Wynik> {
    wykryj_ze_znanymi(probki, fs, &[])
}

/// `znane`: tony spoza listy Quansheng już wcześniej pewnie wykryte; traktowane jak tony z listy.
pub fn wykryj_ze_znanymi(probki: &[f32], fs: f32, znane: &[f32]) -> Option<Wynik> {
    let (x, fs_a) = decymuj(probki, fs);
    let w = widma(&x, fs_a);
    let (ramki, bin_od) = (&w.ramki, w.bin_od);
    if ramki.is_empty() {
        return None;
    }
    let df = fs_a / NFFT as f32;
    let mut siatka = Vec::new();
    let mut f0 = F0_OD;
    while f0 < F0_DO {
        siatka.push(f0);
        f0 *= 1.0 + F0_KROK_WZGL;
    }
    let grzebienie: Vec<Grzebien> = ramki
        .iter()
        .map(|r| grzebien_ramki(r, bin_od, df, &siatka))
        .collect();
    let suma_sil: f32 = grzebienie.iter().map(Grzebien::sila).sum();
    if suma_sil <= 0.0 {
        return None;
    }
    let punktuj = |k: Option<f32>| -> f32 {
        let g: f32 = grzebienie.iter().map(|g| g.zgodnosc(k.unwrap_or(0.0))).sum();
        g / suma_sil + WAGA_OBWIEDNI * obwiednia(&w, df, k)
    };
    let liczba = ((SKAN_DO_HZ - SKAN_OD_HZ) / SKAN_KROK_HZ) as usize + 1;
    let punkty: Vec<(Option<f32>, f32)> = std::iter::once((None, punktuj(None)))
        .chain((0..liczba).map(|i| {
            let k = SKAN_OD_HZ + i as f32 * SKAN_KROK_HZ;
            (Some(k), punktuj(Some(k)))
        }))
        .collect();

    let naj = *punkty.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    let rywal = punkty
        .iter()
        .filter(|p| match (p.0, naj.0) {
            (Some(a), Some(b)) => (a - b).abs() > LISTEK_HZ,
            (None, None) => false,
            _ => true,
        })
        .map(|p| p.1)
        .fold(f32::MIN, f32::max);
    let przewaga = naj.1 - rywal;
    let lista: Vec<f32> = TONY_QUANSHENG.iter().chain(&TONY_CZESTE).chain(znane).copied().collect();
    let ton = naj.0.map(|t| {
        lista
            .iter()
            .copied()
            .find(|q| (q - t).abs() <= PRZYCIAGANIE_HZ)
            .unwrap_or(t)
    });
    let punkty_tonow: Vec<f32> = TONY_QUANSHENG.iter().map(|&q| punktuj(Some(q))).collect();
    let na_liscie = ton.is_none_or(|t| lista.contains(&t));
    let przewaga_listy = if na_liscie {
        let rywal_listy = lista
            .iter()
            .filter(|&&q| ton != Some(q))
            .map(|&q| punktuj(Some(q)))
            .chain(ton.is_some().then(|| punktuj(None)))
            .fold(f32::MIN, f32::max);
        naj.1 - rywal_listy
    } else {
        0.0
    };
    let sila = suma_sil / grzebienie.len() as f32;
    let mut po_f0: Vec<(f32, f32)> = grzebienie.iter().map(|g| (g.f0, g.sila())).collect();
    po_f0.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut narast = 0.0;
    let f0_mediana = po_f0
        .iter()
        .find(|(_, s)| {
            narast += s;
            narast >= suma_sil / 2.0
        })
        .map_or(0.0, |p| p.0);
    let przewaga_aliasu = match naj.0 {
        Some(t) => {
            let alias = punkty
                .iter()
                .filter_map(|&(k, p)| k.map(|k| ((k - t).abs(), p)))
                .filter(|&(d, _)| d >= ALIAS_OD * f0_mediana && d <= ALIAS_DO * f0_mediana)
                .map(|(_, p)| p)
                .fold(f32::MIN, f32::max);
            naj.1 - alias
        }
        None => f32::MAX,
    };
    let pewny = naj.1 >= PROG_PUNKTOW
        && przewaga >= PROG_PRZEWAGI
        && grzebienie.len() >= MIN_RAMEK;
    let glos = naj.1 >= PROG_PUNKTOW && sila >= PROG_SILY;
    let wstepny = if na_liscie {
        glos && przewaga_listy >= PROG_PRZEWAGI_LISTY && grzebienie.len() >= MIN_RAMEK_WSTEPNIE
    } else {
        pewny && glos
    };
    Some(Wynik {
        ton,
        punkty: naj.1,
        pewny,
        wstepny,
        sila,
        przewaga,
        ramek: grzebienie.len(),
        punkty_tonow,
        przewaga_listy,
        skan: punkty,
        f0_mediana,
        przewaga_aliasu,
    })
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::dsp::odwroc;

    /// Głos syntetyczny: harmoniczne F0 z wolnym vibrato i przerwami.
    fn glos(fs: f32, sekundy: f32, f0: f32) -> Vec<f32> {
        let n = (fs * sekundy) as usize;
        let mut faza = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / fs;
                faza += 2.0 * PI * f0 * (1.0 + 0.15 * (2.0 * PI * 0.7 * t).sin()) / fs;
                let suma: f32 = (1..=25)
                    .map(|k| (k as f32 * faza).sin() / (1.0 + k as f32 * 0.2))
                    .sum();
                let bramka = if (2.0 * PI * 1.3 * t).sin() > -0.4 { 0.1 } else { 0.0 };
                suma * bramka
            })
            .collect()
    }

    #[test]
    fn znajduje_kazdy_ton_quansheng() {
        let fs = 16000.0;
        for f0 in [105.0, 190.0] {
            let x = glos(fs, 8.0, f0);
            for ton in TONY_QUANSHENG {
                let w = wykryj(&odwroc(&x, fs, ton), fs).unwrap();
                assert!(w.pewny, "f0={f0} ton={ton} {w:?}");
                assert_eq!(w.ton, Some(ton), "f0={f0} {w:?}");
            }
        }
    }

    #[test]
    fn nosna_spoza_listy_i_brak_inwersji() {
        let fs = 16000.0;
        let x = glos(fs, 8.0, 130.0);
        for nosna in [2632.0, 3729.0, 4096.0] {
            let w = wykryj(&odwroc(&x, fs, nosna), fs).unwrap();
            assert!(w.pewny && (w.ton.unwrap() - nosna).abs() <= 10.0, "{nosna} {:?}", (w.ton, w.punkty, w.przewaga));
        }
        let w = wykryj(&x, fs).unwrap();
        assert!(w.pewny && w.ton.is_none(), "{w:?}");
    }

    #[test]
    fn za_krotko_nie_jest_pewne() {
        let fs = 16000.0;
        let x = glos(fs, 0.8, 130.0);
        assert!(!wykryj(&odwroc(&x, fs, 3000.0), fs).is_some_and(|w| w.pewny));
    }
}
