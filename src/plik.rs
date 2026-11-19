//! Odczyt plików dźwiękowych (WAV, MP3, FLAC, OGG) i zapis WAV.

use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Zwraca próbki mono (średnia kanałów) i częstotliwość próbkowania.
pub fn wczytaj(sciezka: &Path) -> Result<(Vec<f32>, f32), String> {
    let plik = std::fs::File::open(sciezka).map_err(|e| format!("{}: {e}", sciezka.display()))?;
    let mss = MediaSourceStream::new(Box::new(plik), Default::default());
    let mut hint = Hint::new();
    if let Some(r) = sciezka.extension().and_then(|e| e.to_str()) {
        hint.with_extension(r);
    }
    let sonda = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("nieznany format: {e}"))?;
    let mut format = sonda.format;
    let sciezka_audio = format.default_track().ok_or("brak ścieżki audio")?;
    let id = sciezka_audio.id;
    let mut fs = sciezka_audio.codec_params.sample_rate;
    let mut dekoder = symphonia::default::get_codecs()
        .make(&sciezka_audio.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("kodek: {e}"))?;

    let mut wyj = Vec::new();
    let mut bufor: Option<SampleBuffer<f32>> = None;
    while let Ok(pakiet) = format.next_packet() {
        if pakiet.track_id() != id {
            continue;
        }
        let Ok(audio) = dekoder.decode(&pakiet) else {
            continue;
        };
        let spec = *audio.spec();
        fs = Some(spec.rate);
        let kanaly = spec.channels.count();
        let b = bufor.get_or_insert_with(|| SampleBuffer::new(audio.capacity() as u64, spec));
        b.copy_interleaved_ref(audio);
        wyj.extend(
            b.samples()
                .chunks_exact(kanaly)
                .map(|r| r.iter().sum::<f32>() / kanaly as f32),
        );
    }
    let fs = fs.ok_or("nieznana częstotliwość próbkowania")? as f32;
    if wyj.is_empty() {
        return Err("plik bez próbek".into());
    }
    Ok((wyj, fs))
}

pub fn zapisz_wav(sciezka: &Path, probki: &[f32], fs: f32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: fs as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let szczyt = probki.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let wzm = if szczyt > 0.99 { 0.99 / szczyt } else { 1.0 };
    let mut w = hound::WavWriter::create(sciezka, spec).map_err(|e| e.to_string())?;
    for v in probki {
        w.write_sample((v * wzm * 32767.0) as i16).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}
