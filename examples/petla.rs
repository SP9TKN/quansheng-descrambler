//! Test toru na żywo przez kable wirtualne: odtwarza plik na jedno urządzenie i nagrywa z drugiego.
//! petla <plik> <wyjście> <wejście> <nagranie.wav>

use cpal::traits::{DeviceTrait, StreamTrait};
use quansheng_descrambler::{na_zywo, plik};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [p_wej, n_wyj, n_wej, p_wyj] = a.as_slice() else {
        return Err("petla <plik> <wyjście> <wejście> <nagranie.wav>".into());
    };
    let (x, fs) = plik::wczytaj(p_wej.as_ref())?;
    let d_wyj = na_zywo::znajdz_wyjscie(n_wyj)?;
    let d_wej = na_zywo::znajdz_wejscie(n_wej)?;
    let k_wyj = d_wyj.default_output_config().map_err(|e| e.to_string())?;
    let k_wej = d_wej.default_input_config().map_err(|e| e.to_string())?;
    let kanaly_wyj = k_wyj.channels() as usize;
    let kanaly_wej = k_wej.channels() as usize;
    let krok = fs as f64 / k_wyj.sample_rate() as f64;
    let fs_nagr = k_wej.sample_rate() as f32;
    println!("odtwarzanie {} Hz, nagrywanie {} Hz", k_wyj.sample_rate(), k_wej.sample_rate());

    let koniec = Arc::new(AtomicBool::new(false));
    let k2 = koniec.clone();
    let mut poz = 0f64;
    let s_wyj = d_wyj
        .build_output_stream::<f32, _, _>(
            k_wyj.config(),
            move |dane: &mut [f32], _| {
                for r in dane.chunks_exact_mut(kanaly_wyj) {
                    let i = poz as usize;
                    let v = if i + 1 < x.len() {
                        x[i] + (x[i + 1] - x[i]) * (poz - i as f64) as f32
                    } else {
                        k2.store(true, Ordering::Relaxed);
                        0.0
                    };
                    r.fill(v);
                    poz += krok;
                }
            },
            |e| eprintln!("wyjście: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    let nagranie = Arc::new(Mutex::new(Vec::new()));
    let n2 = nagranie.clone();
    let s_wej = d_wej
        .build_input_stream::<f32, _, _>(
            k_wej.config(),
            move |dane: &[f32], _| {
                let mut n = n2.lock().unwrap();
                n.extend(dane.chunks_exact(kanaly_wej).map(|r| r.iter().sum::<f32>() / kanaly_wej as f32));
            },
            |e| eprintln!("wejście: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    s_wej.play().map_err(|e| e.to_string())?;
    s_wyj.play().map_err(|e| e.to_string())?;
    while !koniec.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(1000));
    drop(s_wej);
    drop(s_wyj);
    let n = nagranie.lock().unwrap();
    plik::zapisz_wav(p_wyj.as_ref(), &n, fs_nagr)
}
