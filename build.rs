//! Wbudowuje ikonę w exe okienkowy. Wymaga windres, który jest w tym samym mingw64 co dlltool.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/ikona.rc");
    println!("cargo:rerun-if-changed=assets/ikona.ico");
    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    let wyj = Path::new(&std::env::var("OUT_DIR").unwrap()).join("ikona.o");
    let status = Command::new("windres")
        .args(["--include-dir", "assets", "-O", "coff", "-i", "assets/ikona.rc", "-o"])
        .arg(&wyj)
        .status()
        .expect("brak windres: dodaj C:\\msys64\\mingw64\\bin do PATH");
    assert!(status.success(), "windres nie zbudował ikony");
    println!("cargo:rustc-link-arg-bin=decoder_scrambler_pogsag={}", wyj.display());
}
