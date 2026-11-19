#![windows_subsystem = "windows"]

use eframe::egui;
use quansheng_descrambler::dsp::{self, TONY_QUANSHENG, TONY_RECZNE};
use quansheng_descrambler::na_zywo::{self, Silnik, Tryb, WpisPocsag, Wspolne};
use quansheng_descrambler::{plik, pocsag, wykrywacz};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ZIELONY: egui::Color32 = egui::Color32::from_rgb(70, 190, 110);
const ZOLTY: egui::Color32 = egui::Color32::from_rgb(225, 180, 70);
const CZERWONY: egui::Color32 = egui::Color32::from_rgb(230, 90, 80);

struct Aplikacja {
    wejscia: Vec<String>,
    wyjscia: Vec<String>,
    wej: String,
    wyj: String,
    wspolne: Arc<Wspolne>,
    silnik: Option<Silnik>,
    blad_startu: Option<String>,
    komunikat_pliku: Arc<Mutex<String>>,
    /// Nośna do ustawienia ręcznie, także spoza listy Quansheng.
    dowolny_ton: f32,
}

fn plik_ustawien() -> Option<PathBuf> {
    let mut p = PathBuf::from(std::env::var_os("APPDATA")?);
    p.push("descrambler-quansheng.txt");
    Some(p)
}

/// Ostatnio użyte urządzenia: wejście w pierwszej linii, wyjście w drugiej.
fn zapamietane() -> (Option<String>, Option<String>) {
    let tekst = plik_ustawien().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let mut linie = tekst.lines().map(str::to_string);
    (linie.next(), linie.next())
}

impl Aplikacja {
    fn new() -> Self {
        let (dom_wej, dom_wyj) = na_zywo::domyslne();
        let (zap_wej, zap_wyj) = zapamietane();
        let wejscia = na_zywo::urzadzenia_wejsciowe();
        let wyjscia = na_zywo::urzadzenia_wyjsciowe();
        // Zapamiętane urządzenie mogło zostać odłączone: wtedy domyślne systemowe.
        let wej = zap_wej.filter(|n| wejscia.contains(n)).or(dom_wej);
        let wyj = zap_wyj.filter(|n| wyjscia.contains(n)).or(dom_wyj);
        Self {
            wejscia,
            wyjscia,
            wej: wej.unwrap_or_default(),
            wyj: wyj.unwrap_or_default(),
            wspolne: Wspolne::new(),
            silnik: None,
            blad_startu: None,
            komunikat_pliku: Arc::default(),
            dowolny_ton: 3000.0,
        }
    }

    fn przelacz(&mut self) {
        if self.silnik.take().is_some() {
            return;
        }
        let wynik = na_zywo::znajdz_wejscie(&self.wej).and_then(|we| {
            let wy = na_zywo::znajdz_wyjscie(&self.wyj)?;
            Silnik::start(&we, &wy, self.wspolne.clone())
        });
        match wynik {
            Ok(s) => {
                self.silnik = Some(s);
                self.blad_startu = None;
                if let Some(p) = plik_ustawien() {
                    let _ = std::fs::write(p, format!("{}\n{}\n", self.wej, self.wyj));
                }
            }
            Err(e) => self.blad_startu = Some(e),
        }
    }

    fn odszyfruj_plik(&self) {
        let Some(sciezka) = rfd::FileDialog::new()
            .add_filter("Dźwięk", &["wav", "mp3", "flac", "ogg"])
            .pick_file()
        else {
            return;
        };
        let reczny = match self.wspolne.tryb() {
            Tryb::Reczny(f) => Some(f),
            _ => None,
        };
        let kom = self.komunikat_pliku.clone();
        *kom.lock().unwrap() = "Przetwarzam...".to_string();
        std::thread::spawn(move || {
            *kom.lock().unwrap() = przetworz_plik(sciezka, reczny).unwrap_or_else(|e| e);
        });
    }

    fn pocsag_z_pliku(&self) {
        let Some(sciezka) = rfd::FileDialog::new()
            .add_filter("Dźwięk", &["wav", "mp3", "flac", "ogg"])
            .pick_file()
        else {
            return;
        };
        let kom = self.komunikat_pliku.clone();
        let wspolne = self.wspolne.clone();
        *kom.lock().unwrap() = "Szukam POCSAG w pliku...".to_string();
        std::thread::spawn(move || {
            *kom.lock().unwrap() = przeszukaj_pocsag(&sciezka, &wspolne).unwrap_or_else(|e| e);
        });
    }

    fn pocsag(&self, ui: &mut egui::Ui) {
        let lista = self.wspolne.pocsag.lock().unwrap().clone();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("POCSAG").strong());
            ui.label(egui::RichText::new("512/1200/2400 bit/s").weak());
            if ui.button("Z pliku...").clicked() {
                self.pocsag_z_pliku();
            }
            if ui.add_enabled(!lista.is_empty(), egui::Button::new("Wyczyść")).clicked() {
                self.wspolne.pocsag.lock().unwrap().clear();
            }
        });
        ui.label(egui::RichText::new("Odebrane na żywo zapisują się do POCSAG_RRRR-MM-DD.txt obok programu").weak().small());
        if lista.is_empty() {
            let tekst = if self.silnik.is_some() { "Nasłuch: na razie brak wiadomości" } else { "Start włącza nasłuch" };
            ui.label(egui::RichText::new(tekst).weak());
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for wpis in lista.iter().rev() {
                wpis_pocsag(ui, wpis);
                ui.separator();
            }
        });
    }

    fn status(&self, ui: &mut egui::Ui) {
        let p = self.wspolne.podglad.lock().unwrap();
        let (tekst, kolor) = if self.silnik.is_none() {
            ("Zatrzymany".to_string(), ui.visuals().weak_text_color())
        } else {
            match self.wspolne.tryb() {
                Tryb::Wylaczony => ("Bez odwracania".to_string(), ZOLTY),
                Tryb::Reczny(f) => (format!("Ton ręczny: {f:.0} Hz"), ZIELONY),
                Tryb::Auto => match p.zatwierdzony {
                    Some(Some(f)) if TONY_QUANSHENG.contains(&f) => (format!("Wykryty ton: {f:.0} Hz"), ZIELONY),
                    Some(Some(f)) => (format!("Wykryty ton: {f:.0} Hz (spoza listy Quansheng)"), ZIELONY),
                    Some(None) => ("Mowa bez scramblera".to_string(), ZOLTY),
                    None => ("Szukam tonu: potrzebna chwila mowy".to_string(), ZOLTY),
                },
            }
        };
        ui.label(egui::RichText::new(tekst).size(22.0).color(kolor).strong());
        if self.silnik.is_some() {
            // 300 ramek co 16 ms: ok. 5 s mowy bez rozstrzygnięcia.
            if let Some(w) = p.ostatni.as_ref().filter(|w| !w.pewny && w.ramek >= 300) {
                ui.colored_label(
                    ZOLTY,
                    format!("Ostatnia analiza niejednoznaczna (punkty {:.2}): inny typ scramblera?", w.punkty),
                );
            }
            ui.add(egui::ProgressBar::new((p.poziom * 1.5).min(1.0)).text("Poziom wejścia"));
            if p.poziom >= 0.99 {
                ui.colored_label(CZERWONY, "Wejście przesterowane: przycisz odbiornik");
            }
        }
        if let Some(b) = &p.blad {
            ui.colored_label(CZERWONY, b);
        }
    }

    fn tony(&mut self, ui: &mut egui::Ui) {
        let tryb = self.wspolne.tryb();
        ui.horizontal(|ui| {
            if ui.selectable_label(tryb == Tryb::Auto, "Automat").clicked() {
                self.wspolne.ustaw_tryb(Tryb::Auto);
            }
            if ui.selectable_label(tryb == Tryb::Wylaczony, "Bez odwracania").clicked() {
                self.wspolne.ustaw_tryb(Tryb::Wylaczony);
            }
        });
        ui.horizontal(|ui| {
            let wlasny = matches!(tryb, Tryb::Reczny(f) if !TONY_RECZNE.contains(&f));
            if ui.selectable_label(wlasny, "Dowolny ton").clicked() {
                self.wspolne.ustaw_tryb(Tryb::Reczny(self.dowolny_ton));
            }
            let pole = ui.add(
                egui::DragValue::new(&mut self.dowolny_ton)
                    .range(wykrywacz::SKAN_OD_HZ..=wykrywacz::SKAN_DO_HZ)
                    .speed(1.0)
                    .suffix(" Hz"),
            );
            // Zmiana w polu działa od razu w trybie ręcznym, także po starcie od tonu z listy.
            if pole.changed() && matches!(tryb, Tryb::Reczny(_)) {
                self.wspolne.ustaw_tryb(Tryb::Reczny(self.dowolny_ton));
            }
        });
        ui.add_space(4.0);
        let punkty: Vec<f32> = {
            let p = self.wspolne.podglad.lock().unwrap();
            TONY_RECZNE
                .iter()
                .map(|&t| p.ostatni.as_ref().map_or(0.0, |w| w.punkty_przy(t).clamp(0.0, 1.0)))
                .collect()
        };
        // Dwie kolumny: po lewej tony Quansheng, po prawej pośrednie; okno nie rośnie.
        let szer_paska = ((ui.available_width() - 150.0) / 2.0).max(60.0);
        egui::Grid::new("tony").num_columns(4).spacing([8.0, 3.0]).show(ui, |ui| {
            for (rzad, tony) in TONY_RECZNE.chunks(2).enumerate() {
                for (i, &t) in tony.iter().enumerate() {
                    if ui.selectable_label(tryb == Tryb::Reczny(t), format!("{t:.0} Hz")).clicked() {
                        self.wspolne.ustaw_tryb(Tryb::Reczny(t));
                    }
                    ui.add(egui::ProgressBar::new(punkty[2 * rzad + i]).desired_width(szer_paska).desired_height(14.0));
                }
                ui.end_row();
            }
        });
    }
}

fn przetworz_plik(sciezka: PathBuf, reczny: Option<f32>) -> Result<String, String> {
    let (x, fs) = plik::wczytaj(&sciezka)?;
    let wynik = wykrywacz::wykryj(&x, fs).ok_or("Plik bez sygnału")?;
    let ton = match reczny {
        Some(f) => f,
        None if !wynik.pewny => {
            let kandydaci: Vec<String> = wynik
                .szczyty(2)
                .iter()
                .map(|(t, _)| t.map_or("bez inwersji".to_string(), |f| format!("{f:.0} Hz")))
                .collect();
            return Err(format!(
                "Nie rozpoznano tonu na pewno; najbliżej: {}. Wybierz ton ręcznie i spróbuj ponownie.",
                kandydaci.join(" albo ")
            ));
        }
        None => match wynik.ton {
            Some(f) => f,
            None => return Ok("Ten plik nie ma scramblera: to zwykła mowa.".to_string()),
        },
    };
    let mut wyj = sciezka.clone();
    let nazwa = sciezka.file_stem().unwrap_or_default().to_string_lossy();
    wyj.set_file_name(format!("{nazwa}_odszyfrowane.wav"));
    plik::zapisz_wav(&wyj, &dsp::odwroc(&x, fs, ton), fs)?;
    Ok(format!("Ton {ton:.0} Hz, zapisano: {}", wyj.display()))
}

fn przeszukaj_pocsag(sciezka: &Path, wspolne: &Wspolne) -> Result<String, String> {
    let (x, fs) = plik::wczytaj(sciezka)?;
    let nazwa = sciezka.file_name().unwrap_or_default().to_string_lossy().to_string();
    let lista = pocsag::dekoduj_plik(&x, fs);
    let n = lista.len();
    for w in lista {
        let kiedy = format!("{:.1} s", w.probka as f32 / fs);
        wspolne.dodaj_pocsag(WpisPocsag { kiedy, zrodlo: nazwa.clone(), w });
    }
    Ok(format!("POCSAG: {n} {} w {nazwa}", if n == 1 { "wiadomość" } else { "wiadomości" }))
}

/// Wszystko, co niesie POCSAG: odbiorca, funkcja, treść w obu odczytach i jakość odbioru.
fn wpis_pocsag(ui: &mut egui::Ui, wpis: &WpisPocsag) {
    let w = &wpis.w;
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(&wpis.kiedy).strong());
        ui.label(egui::RichText::new(format!("RIC {}", w.ric)).strong());
        ui.label(format!("funkcja {}", w.funkcja));
        ui.label(format!("{} bit/s", w.predkosc));
        ui.label(egui::RichText::new(&wpis.zrodlo).weak());
    });
    let (glowna, druga) = w.odczyty();
    ui.label(egui::RichText::new(glowna).size(17.0).color(ZIELONY));
    if let Some(d) = druga {
        ui.label(egui::RichText::new(d).weak().small());
    }
    let kolor = if w.bledne_slowa > 0 || !w.zakonczona { ZOLTY } else { ui.visuals().weak_text_color() };
    ui.colored_label(kolor, egui::RichText::new(w.jakosc()).small());
}

fn wybor(ui: &mut egui::Ui, etykieta: &str, opcje: &[String], wartosc: &mut String, wlaczony: bool) {
    ui.label(etykieta);
    ui.add_enabled_ui(wlaczony, |ui| {
        egui::ComboBox::from_id_salt(etykieta)
            .selected_text(wartosc.as_str())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for o in opcje {
                    ui.selectable_value(wartosc, o.clone(), o);
                }
            });
    });
    ui.end_row();
}

impl eframe::App for Aplikacja {
    fn ui(&mut self, ui: &mut egui::Ui, _ramka: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        egui::CentralPanel::default().show(ui, |ui| {
            let dziala = self.silnik.is_some();
            egui::Grid::new("urzadzenia").num_columns(2).min_col_width(60.0).show(ui, |ui| {
                wybor(ui, "Wejście", &self.wejscia, &mut self.wej, !dziala);
                wybor(ui, "Wyjście", &self.wyjscia, &mut self.wyj, !dziala);
            });
            ui.add_space(4.0);
            if ui.button(if dziala { "Zatrzymaj" } else { "Start" }).clicked() {
                self.przelacz();
            }
            if let Some(b) = &self.blad_startu {
                ui.colored_label(CZERWONY, b);
            }
            ui.separator();
            self.status(ui);
            ui.separator();
            self.tony(ui);
            ui.separator();
            if ui.button("Odszyfruj plik...").clicked() {
                self.odszyfruj_plik();
            }
            let kom = self.komunikat_pliku.lock().unwrap().clone();
            if !kom.is_empty() {
                ui.label(kom);
            }
            ui.separator();
            self.pocsag(ui);
        });
    }
}

fn main() -> eframe::Result {
    let ikona = egui::IconData {
        rgba: include_bytes!("../assets/ikona_64.rgba").to_vec(),
        width: 64,
        height: 64,
    };
    let opcje = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([500.0, 860.0]).with_icon(ikona),
        ..Default::default()
    };
    eframe::run_native(
        "decoder_scrambler_pogsag",
        opcje,
        Box::new(|_| Ok(Box::new(Aplikacja::new()))),
    )
}
