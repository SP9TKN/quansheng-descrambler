use quansheng_descrambler::na_zywo::{self, Silnik, Wspolne};
use quansheng_descrambler::{dsp, plik, pocsag, wykrywacz};
use std::path::Path;
use std::time::Duration;

const POMOC: &str = "\
Użycie:
  descrambler-cli wykryj <plik>                         wykrywa ton scramblera
  descrambler-cli odwroc <plik> <wyj.wav> [--ton HZ]    bez --ton wykrywa sam
  descrambler-cli symuluj <plik>                        decyzje automatu krok po kroku, jak na żywo
  descrambler-cli pocsag <plik>                         wiadomości POCSAG 512/1200/2400 bit/s
  descrambler-cli urzadzenia                            lista wejść i wyjść
  descrambler-cli na-zywo <wejście> <wyjście> <sekundy> tryb automatyczny, urządzenia po fragmencie nazwy, też POCSAG";

fn opis(t: Option<f32>) -> String {
    t.map_or("brak inwersji".to_string(), |f| format!("{f:.0} Hz"))
}

fn dzialaj(a: &[String]) -> Result<(), String> {
    let ton_arg = a
        .iter()
        .position(|x| x == "--ton")
        .and_then(|i| a.get(i + 1))
        .map(|s| s.parse::<f32>().map_err(|e| format!("--ton: {e}")))
        .transpose()?;
    match a.first().map(String::as_str) {
        Some("wykryj") => {
            let (x, fs) = plik::wczytaj(Path::new(a.get(1).ok_or(POMOC)?))?;
            let w = wykrywacz::wykryj(&x, fs).ok_or("brak sygnału")?;
            for (t, p) in dsp::TONY_QUANSHENG.iter().zip(&w.punkty_tonow) {
                println!("{t:>6.0} Hz  {p:7.3}");
            }
            // Szczyty skanu: widać, czy rywal leży o F0 od zwycięzcy.
            let opis_szczytow: Vec<String> =
                w.szczyty(6).iter().map(|(f, p)| format!("{}:{p:.2}", opis(*f))).collect();
            println!("SKAN {} | F0 {:.0} Hz", opis_szczytow.join("  "), w.f0_mediana);
            println!(
                "WYNIK {} {} punkty {:.3} przewaga {:.3} alias {:.3} ramek {} fs {fs:.0}",
                if w.pewny { "pewny" } else { "NIEPEWNY" },
                opis(w.ton),
                w.punkty,
                w.przewaga,
                w.przewaga_aliasu.min(9.0),
                w.ramek
            );
            Ok(())
        }
        Some("odwroc") => {
            let (x, fs) = plik::wczytaj(Path::new(a.get(1).ok_or(POMOC)?))?;
            let wyj = a.get(2).ok_or(POMOC)?;
            let ton = match ton_arg {
                Some(t) => Some(t),
                None => {
                    let w = wykrywacz::wykryj(&x, fs).ok_or("brak sygnału")?;
                    println!("wykryto: {} (punkty {:.2})", opis(w.ton), w.punkty);
                    if !w.pewny {
                        let k: Vec<String> = w.szczyty(2).iter().map(|(t, _)| opis(*t)).collect();
                        return Err(format!("wynik niepewny ({}): podaj ton ręcznie (--ton)", k.join(" albo ")));
                    }
                    w.ton
                }
            };
            let y = match ton {
                Some(t) => dsp::odwroc(&x, fs, t),
                None => x,
            };
            plik::zapisz_wav(Path::new(wyj), &y, fs)
        }
        Some("symuluj") => {
            let (x, fs) = plik::wczytaj(Path::new(a.get(1).ok_or(POMOC)?))?;
            let mut an = na_zywo::Analizator::new(fs);
            let kawalek = (fs * 0.2) as usize;
            for (i, k) in x.chunks(kawalek).enumerate() {
                if let Some(w) = an.dodaj(k) {
                    println!(
                        "{:5.1} s  {:>14} {} punkty {:.3} przewaga {:.3} ramek {:3} -> ton {:?}",
                        (i + 1) as f32 * 0.2,
                        opis(w.ton),
                        if w.pewny { "pewny   " } else { "niepewny" },
                        w.punkty,
                        w.przewaga,
                        w.ramek,
                        an.zatwierdzony
                    );
                }
            }
            Ok(())
        }
        Some("pocsag") => {
            let (x, fs) = plik::wczytaj(Path::new(a.get(1).ok_or(POMOC)?))?;
            let lista = pocsag::dekoduj_plik(&x, fs);
            for w in &lista {
                println!(
                    "{:8.2} s  {:4} bit/s  RIC {:>7}  funkcja {}  poprawione bity {}  błędne słowa {}{}",
                    w.probka as f32 / fs,
                    w.predkosc,
                    w.ric,
                    w.funkcja,
                    w.poprawki,
                    w.bledne_slowa,
                    if w.zakonczona { "" } else { "  (urwana)" }
                );
                println!("           tekst: {}", w.tekst());
                println!("           cyfry: {}", w.cyfry());
            }
            println!("wiadomości: {}", lista.len());
            Ok(())
        }
        Some("urzadzenia") => {
            println!("WEJŚCIA:");
            na_zywo::urzadzenia_wejsciowe().iter().for_each(|n| println!("  {n}"));
            println!("WYJŚCIA:");
            na_zywo::urzadzenia_wyjsciowe().iter().for_each(|n| println!("  {n}"));
            Ok(())
        }
        Some("na-zywo") => {
            let we = na_zywo::znajdz_wejscie(a.get(1).ok_or(POMOC)?)?;
            let wy = na_zywo::znajdz_wyjscie(a.get(2).ok_or(POMOC)?)?;
            let sekundy: u32 = a.get(3).ok_or(POMOC)?.parse().map_err(|_| POMOC)?;
            let wspolne = Wspolne::new();
            let _silnik = Silnik::start(&we, &wy, wspolne.clone())?;
            let mut poprzedni = None;
            let mut wypisane = 0;
            for s in 0..sekundy * 2 {
                std::thread::sleep(Duration::from_millis(500));
                for wp in wspolne.pocsag.lock().unwrap().iter().skip(wypisane) {
                    println!(
                        "{:5.1} s  POCSAG {}  {} bit/s  RIC {}  funkcja {}: {}",
                        s as f32 / 2.0,
                        wp.kiedy,
                        wp.w.predkosc,
                        wp.w.ric,
                        wp.w.funkcja,
                        wp.w.tekst()
                    );
                    wypisane += 1;
                }
                let p = wspolne.podglad.lock().unwrap();
                if p.zatwierdzony != poprzedni {
                    poprzedni = p.zatwierdzony;
                    println!("{:5.1} s  ton: {}", s as f32 / 2.0, opis(poprzedni.flatten()));
                }
                if let Some(b) = &p.blad {
                    return Err(b.clone());
                }
            }
            let p = wspolne.podglad.lock().unwrap();
            println!("koniec, fs wej {} wyj {}, ostatni wynik: {:?}", p.fs_wej, p.fs_wyj,
                p.ostatni.as_ref().map(|w| (w.ton, w.pewny, w.punkty, w.ramek)));
            Ok(())
        }
        _ => Err(POMOC.to_string()),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = dzialaj(&a) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
