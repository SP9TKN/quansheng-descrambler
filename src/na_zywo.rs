//! Praca na żywo: wejście karty -> inwerter -> wyjście karty, a obok wykrywacz w osobnym wątku.

use crate::dsp::{Inwerter, TONY_QUANSHENG};
use crate::pocsag::{DekoderNaZywo, Wiadomosc};
use crate::wykrywacz::{self, Wynik};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Blok, w którym oceniamy, czy jest sygnał.
const BLOK_S: f32 = 0.1;
const SYGNAL_RMS: f32 = 0.003;
/// Dłuższa cisza to nowe nadanie, może innej stacji: stare próbki nie mogą mieszać się z innym tonem.
const NOWE_NADANIE_S: f32 = 0.6;
const PAMIEC_MOWY_S: f32 = 12.0;
const ANALIZA_CO_S: f32 = 0.25;
const POWTORZEN: u32 = 3;
const MAX_ZNANYCH: usize = 16;
const BUFOR_WYJSCIA_S: f32 = 0.15;
/// Co tyle sekund okno POCSAG jest dekodowane od nowa (ok. 75 ms pracy na okno).
const POCSAG_CO_S: f32 = 1.0;
const MAX_WPISOW_POCSAG: usize = 500;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tryb {
    Auto,
    Reczny(f32),
    Wylaczony,
}

/// Zbiera mowę i decyduje, kiedy zmienić ton. Bez kart dźwiękowych, więc da się to testować.
pub struct Analizator {
    fs: f32,
    blok: Vec<f32>,
    mowa: VecDeque<f32>,
    nowych: usize,
    ciszy: usize,
    kandydat: Option<Option<f32>>,
    powtorzen: u32,
    /// Tony spoza listy Quansheng wykryte w tej sesji.
    znane: Vec<f32>,
    pub zatwierdzony: Option<Option<f32>>,
    pub analiza_co_s: f32,
    /// Licznik przerw kończących rozmowę, do odtwarzania decyzji offline.
    pub rozmow: u32,
}

impl Analizator {
    pub fn new(fs: f32) -> Self {
        Self {
            fs,
            blok: Vec::new(),
            mowa: VecDeque::new(),
            nowych: 0,
            ciszy: 0,
            kandydat: None,
            powtorzen: 0,
            znane: Vec::new(),
            zatwierdzony: None,
            analiza_co_s: ANALIZA_CO_S,
            rozmow: 0,
        }
    }

    /// Zwraca wynik, jeśli te próbki wyzwoliły nową analizę.
    pub fn dodaj(&mut self, probki: &[f32]) -> Option<Wynik> {
        let dl_bloku = (self.fs * BLOK_S) as usize;
        let mut wynik = None;
        self.blok.extend_from_slice(probki);
        let mut start = 0;
        while self.blok.len() - start >= dl_bloku {
            let b = &self.blok[start..start + dl_bloku];
            start += dl_bloku;
            let rms = (b.iter().map(|v| v * v).sum::<f32>() / dl_bloku as f32).sqrt();
            if rms < SYGNAL_RMS {
                self.ciszy += dl_bloku;
                if self.ciszy as f32 >= NOWE_NADANIE_S * self.fs && !self.mowa.is_empty() {
                    self.mowa.clear();
                    self.nowych = 0;
                    self.kandydat = None;
                    self.powtorzen = 0;
                    self.rozmow += 1;
                }
                continue;
            }
            self.ciszy = 0;
            self.mowa.extend(b.iter().copied());
            self.nowych += dl_bloku;
            let limit = (PAMIEC_MOWY_S * self.fs) as usize;
            while self.mowa.len() > limit {
                self.mowa.pop_front();
            }
            if self.nowych as f32 >= self.analiza_co_s * self.fs {
                self.nowych = 0;
                let w = wykrywacz::wykryj_ze_znanymi(self.mowa.make_contiguous(), self.fs, &self.znane);
                if let Some(w) = &w {
                    self.ocen(w);
                }
                wynik = w;
            }
        }
        self.blok.drain(..start);
        wynik
    }

    fn ocen(&mut self, w: &Wynik) {
        if !w.wstepny {
            return;
        }
        self.powtorzen = if self.kandydat == Some(w.ton) { self.powtorzen + 1 } else { 1 };
        self.kandydat = Some(w.ton);
        // Analizy co 0,25 s prawie się pokrywają, więc potwierdzenie musi objąć kilka z nich.
        if self.zatwierdzony != Some(w.ton) && self.powtorzen >= POWTORZEN {
            self.zatwierdzony = Some(w.ton);
            if let Some(t) = w.ton {
                if !TONY_QUANSHENG.contains(&t) && !self.znane.contains(&t) && self.znane.len() < MAX_ZNANYCH {
                    self.znane.push(t);
                }
            }
        }
    }
}

#[derive(Default)]
pub struct Podglad {
    pub ostatni: Option<Wynik>,
    pub zatwierdzony: Option<Option<f32>>,
    pub poziom: f32,
    pub fs_wej: u32,
    pub fs_wyj: u32,
    pub blad: Option<String>,
}

#[derive(Clone)]
pub struct WpisPocsag {
    /// Godzina odbioru albo miejsce w pliku.
    pub kiedy: String,
    /// „na żywo” albo nazwa pliku.
    pub zrodlo: String,
    pub w: Wiadomosc,
}

pub struct Wspolne {
    pub podglad: Mutex<Podglad>,
    /// Poza podglądem, bo Start go zeruje, a wiadomości mają zostać.
    pub pocsag: Mutex<Vec<WpisPocsag>>,
    tryb: Mutex<Tryb>,
    /// Bity f32 aktywnej nośnej; 0 = przepuszczanie bez zmian.
    nosna: AtomicU32,
}

impl Wspolne {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            podglad: Mutex::default(),
            pocsag: Mutex::default(),
            tryb: Mutex::new(Tryb::Auto),
            nosna: AtomicU32::new(0),
        })
    }

    pub fn dodaj_pocsag(&self, wpis: WpisPocsag) {
        let mut l = self.pocsag.lock().unwrap();
        l.push(wpis);
        if l.len() > MAX_WPISOW_POCSAG {
            l.remove(0);
        }
    }

    pub fn tryb(&self) -> Tryb {
        *self.tryb.lock().unwrap()
    }

    pub fn ustaw_tryb(&self, t: Tryb) {
        *self.tryb.lock().unwrap() = t;
        self.zastosuj();
    }

    fn zastosuj(&self) {
        let nosna = match self.tryb() {
            Tryb::Auto => self.podglad.lock().unwrap().zatwierdzony.flatten().unwrap_or(0.0),
            Tryb::Reczny(f) => f,
            Tryb::Wylaczony => 0.0,
        };
        self.nosna.store(nosna.to_bits(), Ordering::Relaxed);
    }
}

pub fn urzadzenia_wejsciowe() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices().map_or(Vec::new(), |l| l.filter_map(|d| nazwa(&d)).collect())
}

pub fn urzadzenia_wyjsciowe() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices().map_or(Vec::new(), |l| l.filter_map(|d| nazwa(&d)).collect())
}

pub fn domyslne() -> (Option<String>, Option<String>) {
    let host = cpal::default_host();
    (
        host.default_input_device().and_then(|d| nazwa(&d)),
        host.default_output_device().and_then(|d| nazwa(&d)),
    )
}

pub fn nazwa(d: &Device) -> Option<String> {
    d.description().ok().map(|o| o.name().to_string())
}

pub fn znajdz_wejscie(fragment: &str) -> Result<Device, String> {
    let host = cpal::default_host();
    let mut l = host.input_devices().map_err(|e| e.to_string())?;
    l.find(|d| nazwa(d).is_some_and(|n| n.contains(fragment)))
        .ok_or_else(|| format!("brak wejścia: {fragment}"))
}

pub fn znajdz_wyjscie(fragment: &str) -> Result<Device, String> {
    let host = cpal::default_host();
    let mut l = host.output_devices().map_err(|e| e.to_string())?;
    l.find(|d| nazwa(d).is_some_and(|n| n.contains(fragment)))
        .ok_or_else(|| format!("brak wyjścia: {fragment}"))
}

/// Lokalna data RRRR-MM-DD i godzina GG:MM:SS sprzed `temu_s` sekund.
pub fn data_i_godzina(temu_s: f32) -> (String, String) {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let t = utc + przesuniecie_strefy(utc) - temu_s.round() as i64;
    let (r, m, d) = data_z_dni(t.div_euclid(86400));
    let s = t.rem_euclid(86400);
    (format!("{r:04}-{m:02}-{d:02}"), format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60))
}

/// Czas lokalny minus UTC w sekundach, z zegara Windows (z czasem letnim).
fn przesuniecie_strefy(utc: i64) -> i64 {
    #[cfg(windows)]
    {
        // SYSTEMTIME: rok, miesiąc, dzień tygodnia, dzień, godzina, minuta, sekunda, ms.
        unsafe extern "system" {
            fn GetLocalTime(czas: *mut u16);
        }
        let mut t = [0u16; 8];
        unsafe { GetLocalTime(t.as_mut_ptr()) };
        let lokalna = t[4] as i64 * 3600 + t[5] as i64 * 60 + t[6] as i64;
        let r = (lokalna - utc.rem_euclid(86400)).rem_euclid(86400);
        // Strefy idą od -12 do +14 h co 15 min; zaokrąglenie gubi sekundę między odczytami zegarów.
        let r = if r > 14 * 3600 { r - 86400 } else { r };
        (r + 450).div_euclid(900) * 900
    }
    #[cfg(not(windows))]
    {
        let _ = utc;
        0
    }
}

/// Dni od 1970-01-01 na datę kalendarza gregoriańskiego (algorytm H. Hinnanta).
fn data_z_dni(dni: i64) -> (i64, u32, u32) {
    let z = dni + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// Dopisuje wiadomość do pliku dnia POCSAG_RRRR-MM-DD.txt w folderze programu.
fn zapisz_w_dzienniku(data: &str, wpis: &WpisPocsag) -> std::io::Result<()> {
    let mut sciezka = std::env::current_exe()?;
    sciezka.set_file_name(format!("POCSAG_{data}.txt"));
    let nowy = !sciezka.exists();
    let mut f = OpenOptions::new().create(true).append(true).open(&sciezka)?;
    if nowy {
        write!(f, "godzina | RIC | funkcja | prędkość | treść | drugi odczyt | jakość\r\n")?;
    }
    let w = &wpis.w;
    let (glowna, druga) = w.odczyty();
    write!(
        f,
        "{} | RIC {} | funkcja {} | {} bit/s | {} | {} | {}\r\n",
        wpis.kiedy,
        w.ric,
        w.funkcja,
        w.predkosc,
        glowna,
        druga.unwrap_or_default(),
        w.jakosc()
    )
}

/// Xrun i podobne to chwilowe zdarzenia: strumień gra dalej, więc to nie jest awaria.
fn zglos_blad(wspolne: &Wspolne, kierunek: &str, e: cpal::Error) {
    if !matches!(e.kind(), ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied) {
        wspolne.podglad.lock().unwrap().blad = Some(format!("{kierunek}: {e}"));
    }
}

struct Wejscie {
    wspolne: Arc<Wspolne>,
    inwerter: Inwerter,
    nosna: u32,
    do_wyjscia: Arc<Mutex<VecDeque<f32>>>,
    do_analizy: Arc<Mutex<Vec<f32>>>,
    kanaly: usize,
    limit_wyjscia: usize,
    bufor: Vec<f32>,
    /// Bieżące maksimum do miernika, opadające między blokami.
    szczyt: f32,
}

impl Wejscie {
    fn przyjmij<T>(&mut self, dane: &[T])
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        self.bufor.clear();
        self.bufor.extend(dane.chunks_exact(self.kanaly).map(|r| {
            r.iter().map(|&s| f32::from_sample(s)).sum::<f32>() / self.kanaly as f32
        }));
        let bity = self.wspolne.nosna.load(Ordering::Relaxed);
        if bity != self.nosna {
            self.nosna = bity;
            if bity != 0 {
                self.inwerter.ustaw_nosna(f32::from_bits(bity));
            }
        }
        self.szczyt *= 0.8;
        for &v in &self.bufor {
            self.szczyt = self.szczyt.max(v.abs());
        }
        if let Ok(mut p) = self.wspolne.podglad.try_lock() {
            p.poziom = self.szczyt;
        }
        self.do_analizy.lock().unwrap().extend_from_slice(&self.bufor);

        let mut q = self.do_wyjscia.lock().unwrap();
        for i in 0..self.bufor.len() {
            let x = self.bufor[i];
            // Inwerter liczy też przy przepuszczaniu, żeby przełączenie nie dawało trzasku z pustej linii.
            let y = self.inwerter.przetworz(x);
            q.push_back(if self.nosna != 0 { y } else { x });
        }
        while q.len() > self.limit_wyjscia {
            q.pop_front();
        }
    }
}

fn strumien_wejscia<T>(
    d: &Device,
    cfg: StreamConfig,
    mut wej: Wejscie,
    wspolne: Arc<Wspolne>,
) -> Result<Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    d.build_input_stream::<T, _, _>(
        cfg,
        move |dane: &[T], _| wej.przyjmij(dane),
        move |e| zglos_blad(&wspolne, "wejście", e),
        None,
    )
    .map_err(|e| format!("wejście: {e}"))
}

struct Wyjscie {
    kolejka: Arc<Mutex<VecDeque<f32>>>,
    /// Próbki wejścia na próbkę wyjścia.
    krok: f64,
    poz: f64,
    cel: usize,
    napelnianie: bool,
    kanaly: usize,
}

impl Wyjscie {
    fn wypelnij<T>(&mut self, dane: &mut [T])
    where
        T: SizedSample + FromSample<f32>,
    {
        let mut q = self.kolejka.lock().unwrap();
        if q.len() >= self.cel {
            self.napelnianie = false;
        }
        // Zegary kart się rozjeżdżają: delikatnie gonimy poziom bufora zamiast gubić próbki.
        let odchylka = (q.len() as f64 - self.cel as f64) / self.cel as f64;
        let krok = self.krok * (1.0 + (odchylka * 0.005).clamp(-0.005, 0.005));
        for ramka in dane.chunks_exact_mut(self.kanaly) {
            let mut y = 0.0;
            if !self.napelnianie {
                while self.poz >= 1.0 && !q.is_empty() {
                    q.pop_front();
                    self.poz -= 1.0;
                }
                if q.len() >= 2 {
                    y = q[0] + (q[1] - q[0]) * self.poz as f32;
                    self.poz += krok;
                } else {
                    self.napelnianie = true;
                }
            }
            ramka.fill(T::from_sample(y.clamp(-1.0, 1.0)));
        }
    }
}

fn strumien_wyjscia<T>(
    d: &Device,
    cfg: StreamConfig,
    mut wyj: Wyjscie,
    wspolne: Arc<Wspolne>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    d.build_output_stream::<T, _, _>(
        cfg,
        move |dane: &mut [T], _| wyj.wypelnij(dane),
        move |e| zglos_blad(&wspolne, "wyjście", e),
        None,
    )
    .map_err(|e| format!("wyjście: {e}"))
}

pub struct Silnik {
    _wejscie: Stream,
    _wyjscie: Stream,
    stop: Arc<AtomicBool>,
}

impl Drop for Silnik {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Silnik {
    pub fn start(d_wej: &Device, d_wyj: &Device, wspolne: Arc<Wspolne>) -> Result<Self, String> {
        let k_wej = d_wej.default_input_config().map_err(|e| format!("wejście: {e}"))?;
        let k_wyj = d_wyj.default_output_config().map_err(|e| format!("wyjście: {e}"))?;
        let fs_wej = k_wej.sample_rate();
        let fs_wyj = k_wyj.sample_rate();
        {
            let mut p = wspolne.podglad.lock().unwrap();
            *p = Podglad { fs_wej, fs_wyj, ..Podglad::default() };
        }
        wspolne.zastosuj();

        let kolejka = Arc::new(Mutex::new(VecDeque::new()));
        let do_analizy = Arc::new(Mutex::new(Vec::new()));
        let wej = Wejscie {
            wspolne: wspolne.clone(),
            inwerter: Inwerter::new(fs_wej as f32, 3000.0),
            nosna: 0,
            do_wyjscia: kolejka.clone(),
            do_analizy: do_analizy.clone(),
            kanaly: k_wej.channels() as usize,
            limit_wyjscia: fs_wej as usize,
            bufor: Vec::new(),
            szczyt: 0.0,
        };
        let cfg = k_wej.config();
        let w = wspolne.clone();
        let s_wej = match k_wej.sample_format() {
            SampleFormat::F32 => strumien_wejscia::<f32>(d_wej, cfg, wej, w),
            SampleFormat::I16 => strumien_wejscia::<i16>(d_wej, cfg, wej, w),
            SampleFormat::I32 => strumien_wejscia::<i32>(d_wej, cfg, wej, w),
            SampleFormat::U8 => strumien_wejscia::<u8>(d_wej, cfg, wej, w),
            f => Err(format!("wejście: nieobsługiwany format {f}")),
        }?;

        let wyj = Wyjscie {
            kolejka,
            krok: fs_wej as f64 / fs_wyj as f64,
            poz: 0.0,
            cel: (fs_wej as f32 * BUFOR_WYJSCIA_S) as usize,
            napelnianie: true,
            kanaly: k_wyj.channels() as usize,
        };
        let cfg = k_wyj.config();
        let w = wspolne.clone();
        let s_wyj = match k_wyj.sample_format() {
            SampleFormat::F32 => strumien_wyjscia::<f32>(d_wyj, cfg, wyj, w),
            SampleFormat::I16 => strumien_wyjscia::<i16>(d_wyj, cfg, wyj, w),
            SampleFormat::I32 => strumien_wyjscia::<i32>(d_wyj, cfg, wyj, w),
            SampleFormat::U8 => strumien_wyjscia::<u8>(d_wyj, cfg, wyj, w),
            f => Err(format!("wyjście: nieobsługiwany format {f}")),
        }?;

        s_wej.play().map_err(|e| format!("wejście: {e}"))?;
        s_wyj.play().map_err(|e| format!("wyjście: {e}"))?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_w = stop.clone();
        // POCSAG w osobnym wątku: okno dekoduje się dłużej, niż trwa analiza mowy.
        // Kończy się sam, gdy wątek analizy zniknie razem z nadawcą kanału.
        let (do_pocsag, z_analizy) = std::sync::mpsc::channel::<Vec<f32>>();
        let wsp = wspolne.clone();
        std::thread::spawn(move || {
            let fs = fs_wej as f32;
            let mut d = DekoderNaZywo::new(fs, POCSAG_CO_S);
            while let Ok(probki) = z_analizy.recv() {
                for w in d.dodaj(&probki) {
                    let temu_s = (d.koniec() - w.probka) as f32 / fs;
                    let (data, kiedy) = data_i_godzina(temu_s);
                    let wpis = WpisPocsag { kiedy, zrodlo: "na żywo".into(), w };
                    if let Err(e) = zapisz_w_dzienniku(&data, &wpis) {
                        wsp.podglad.lock().unwrap().blad = Some(format!("dziennik POCSAG: {e}"));
                    }
                    wsp.dodaj_pocsag(wpis);
                }
            }
        });
        std::thread::spawn(move || {
            let mut a = Analizator::new(fs_wej as f32);
            while !stop_w.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                let probki = std::mem::take(&mut *do_analizy.lock().unwrap());
                let wynik = a.dodaj(&probki);
                let _ = do_pocsag.send(probki);
                if let Some(w) = wynik {
                    let mut p = wspolne.podglad.lock().unwrap();
                    p.ostatni = Some(w);
                    p.zatwierdzony = a.zatwierdzony;
                    drop(p);
                    wspolne.zastosuj();
                }
            }
        });
        Ok(Self { _wejscie: s_wej, _wyjscie: s_wyj, stop })
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::dsp::odwroc;
    use std::f32::consts::PI;

    fn glos(fs: f32, sekundy: f32, f0: f32) -> Vec<f32> {
        let mut faza = 0.0f32;
        (0..(fs * sekundy) as usize)
            .map(|i| {
                let t = i as f32 / fs;
                faza += 2.0 * PI * f0 * (1.0 + 0.15 * (2.0 * PI * 0.7 * t).sin()) / fs;
                let s: f32 = (1..=25).map(|k| (k as f32 * faza).sin() / (1.0 + k as f32 * 0.2)).sum();
                s * 0.1
            })
            .collect()
    }

    fn podaj(a: &mut Analizator, x: &[f32]) {
        for kawalek in x.chunks(1024) {
            a.dodaj(kawalek);
        }
    }

    #[test]
    fn zmiana_tonu_miedzy_rozmowami() {
        let fs = 16000.0;
        let x = glos(fs, 6.0, 120.0);
        let mut a = Analizator::new(fs);
        podaj(&mut a, &odwroc(&x, fs, 3000.0));
        assert_eq!(a.zatwierdzony, Some(Some(3000.0)));
        // Cisza dłuższa niż próg: nowa rozmowa, stare próbki nie wpływają na nowy ton.
        podaj(&mut a, &vec![0.0; (fs * 4.0) as usize]);
        assert_eq!(a.zatwierdzony, Some(Some(3000.0)), "ton trzymany w przerwie");
        podaj(&mut a, &odwroc(&x, fs, 2700.0));
        assert_eq!(a.zatwierdzony, Some(Some(2700.0)));
        podaj(&mut a, &vec![0.0; (fs * 4.0) as usize]);
        podaj(&mut a, &x);
        assert_eq!(a.zatwierdzony, Some(None), "potem nadawanie bez scramblera");
    }

    #[test]
    fn data_z_dni_kalendarz() {
        assert_eq!(data_z_dni(0), (1970, 1, 1));
        assert_eq!(data_z_dni(19_723), (2024, 1, 1));
        assert_eq!(data_z_dni(19_723 + 59), (2024, 2, 29));
        assert_eq!(data_z_dni(20_394), (2025, 11, 2));
        assert_eq!(data_z_dni(-1), (1969, 12, 31));
    }

    #[test]
    fn szum_nie_zmienia_tonu() {
        let fs = 16000.0;
        let mut a = Analizator::new(fs);
        podaj(&mut a, &odwroc(&glos(fs, 6.0, 120.0), fs, 3300.0));
        let mut st = 12345u32;
        let szum: Vec<f32> = (0..(fs * 10.0) as usize)
            .map(|_| {
                st = st.wrapping_mul(1664525).wrapping_add(1013904223);
                (st >> 8) as f32 / (1u32 << 23) as f32 * 0.1 - 0.1
            })
            .collect();
        podaj(&mut a, &vec![0.0; (fs * 4.0) as usize]);
        podaj(&mut a, &szum);
        assert_eq!(a.zatwierdzony, Some(Some(3300.0)));
    }
}
