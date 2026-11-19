//! Zrzut przebiegu analiz do CSV, do strojenia reguł automatu offline.
//! przebieg <krok_s> <wyj.csv> <plik lub katalog>...   Nazwa pliku: <prawda>__<cokolwiek>.wav

use quansheng_descrambler::na_zywo::Analizator;
use quansheng_descrambler::plik;
use std::io::Write;
use std::path::Path;

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let krok: f32 = a[0].parse().map_err(|_| "krok")?;
    let mut csv = std::fs::File::create(&a[1]).map_err(|e| e.to_string())?;
    writeln!(csv, "plik,prawda,t,ton,punkty,przewaga,przewaga_listy,ramek,sila,rozmowa,ms").unwrap();
    let mut pliki: Vec<std::path::PathBuf> = Vec::new();
    for p in &a[2..] {
        if Path::new(p).is_dir() {
            let lista = std::fs::read_dir(p).map_err(|e| e.to_string())?;
            pliki.extend(lista.flatten().map(|e| e.path()).filter(|s| s.extension().is_some_and(|r| r == "wav")));
        } else {
            pliki.push(p.into());
        }
    }
    pliki.sort();
    for p in &pliki {
        let (x, fs) = plik::wczytaj(p)?;
        let nazwa = p.file_stem().unwrap().to_string_lossy().to_string();
        let prawda = nazwa.split("__").next().unwrap().to_string();
        let mut an = Analizator::new(fs);
        an.analiza_co_s = krok;
        let kaw = (fs * 0.05) as usize;
        for (i, k) in x.chunks(kaw).enumerate() {
            let start = std::time::Instant::now();
            if let Some(w) = an.dodaj(k) {
                writeln!(
                    csv,
                    "{nazwa},{prawda},{:.2},{},{:.3},{:.3},{:.3},{},{:.3},{},{:.1}",
                    (i + 1) as f32 * 0.05,
                    w.ton.map_or("brak".to_string(), |t| format!("{t:.0}")),
                    w.punkty,
                    w.przewaga,
                    w.przewaga_listy,
                    w.ramek,
                    w.sila,
                    an.rozmow,
                    start.elapsed().as_secs_f32() * 1000.0
                )
                .unwrap();
            }
        }
    }
    Ok(())
}
