use quansheng_descrambler::{plik, pocsag};
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (x, fs) = plik::wczytaj(std::path::Path::new(&a[0])).unwrap();
    let okno = (fs * 8.0) as usize;
    for start_s in [0.0f32, 100.0, 200.0, 300.0, 385.0, 500.0, 870.0] {
        let s = (start_s * fs) as usize;
        if s + okno > x.len() { continue; }
        let kawalek = &x[s..s + okno];
        let rms = (kawalek.iter().map(|v| v * v).sum::<f32>() / okno as f32).sqrt();
        let mut wiersz = format!("{start_s:6.0} s rms {rms:.4}:");
        for p in pocsag::PREDKOSCI {
            let t = Instant::now();
            let n = pocsag::dekoduj_predkosc(kawalek, fs, p).len();
            wiersz += &format!("  {p}: {:5.1} ms ({n})", t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{wiersz}");
    }
}
