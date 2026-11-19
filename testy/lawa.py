"""Stanowisko testowe wykrywacza: prawdziwa mowa -> tor UV-K5 (niezależna implementacja w scipy)
-> WAV, potem wykrywanie przez descrambler-cli. Argumenty: katalog z mową, ścieżka CLI."""
import glob, os, subprocess, sys, tempfile, wave
import numpy as np
from scipy.signal import firwin, lfilter, resample_poly
from math import gcd

sys.stdout.reconfigure(encoding="utf-8")
MOWA, CLI = sys.argv[1], sys.argv[2]
TONY = list(range(2600, 3600, 100))
SPOZA = [2100, 2632, 2868, 3023, 3339, 3729, 4096]
rng = np.random.default_rng(1)


def czytaj(p):
    w = wave.open(p)
    b = w.readframes(w.getnframes())
    x = np.frombuffer(b, np.int16).astype(float) / 32768 if w.getsampwidth() == 2 else (np.frombuffer(b, np.uint8).astype(float) - 128) / 128
    return x[:: w.getnchannels()], w.getframerate()


def zapisz(p, x, fs):
    w = wave.open(p, "wb")
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(fs)
    w.writeframes((np.clip(x, -1, 1) * 32767).astype(np.int16).tobytes())
    w.close()


def szyfruj(x, fs, ton, snr_db, ctcss, fs_wyj, emfaza=False):
    g = gcd(fs, 48000)
    y = resample_poly(x, 48000 // g, fs // g)
    y = lfilter(firwin(801, [300, 3000], pass_zero=False, fs=48000), 1, y)
    if ton:
        t = np.arange(len(y)) / 48000
        y = 2 * y * np.cos(2 * np.pi * ton * t)
        y = lfilter(firwin(801, 3400, fs=48000), 1, y)
    if emfaza:
        y = np.diff(y, prepend=0.0)
    y = y / (np.sqrt(np.mean(y**2)) + 1e-12) * 0.1
    t = np.arange(len(y)) / 48000
    if ctcss:
        y = y + 0.02 * np.sin(2 * np.pi * ctcss * t)
    if snr_db is not None:
        y = y + rng.normal(0, 0.1 * 10 ** (-snr_db / 20), len(y))
    g = gcd(48000, fs_wyj)
    return resample_poly(y, fs_wyj // g, 48000 // g)


def wykryj(p):
    r = subprocess.run([CLI, "wykryj", p], capture_output=True, text=True, encoding="utf-8")
    for l in r.stdout.splitlines():
        if l.startswith("WYNIK"):
            cz = l.split()
            ton = None if cz[2] == "brak" else int(cz[2])
            return ton, cz[1] == "pewny", float(cz[cz.index("punkty") + 1]), float(cz[cz.index("przewaga") + 1])
    return "blad", False, 0.0, 0.0


pliki = sorted(glob.glob(os.path.join(MOWA, "*.wav")))
dlugie = [p for p in pliki if os.path.getsize(p) > 100000]
krotkie = [p for p in pliki if p not in dlugie]
warianty = [("czysto 48k", None, None, 48000, False), ("SNR 20 + CTCSS 88,5", 20, 88.5, 48000, False),
            ("SNR 10 + CTCSS 88,5", 10, 88.5, 48000, False), ("SNR 5", 5, None, 48000, False),
            ("SNR 20, 8 kHz", 20, None, 8000, False), ("SNR 20, bez de-emfazy", 20, None, 48000, True)]
przypadki = [("mowa 15 s", dlugie, 15.0), ("mowa 3 s", dlugie, 3.0), ("klipy <1 s", krotkie, None)]
wszystkie_przewagi_dobre, wszystkie_zle = [], []
with tempfile.TemporaryDirectory() as d:
    p = os.path.join(d, "t.wav")
    print(f"{'wariant':24s}{'materiał':12s}{'pewne trafne':>14s}{'pewne BŁĘDNE':>14s}{'niepewne':>10s}{'bez inw. ok':>13s}{'spoza: trafne':>15s}{'pewne złe':>11s}")
    spoza_wg_tonu = {}
    for nazwa, snr, ct, fw, em in warianty:
        for mat, zbior, dl in przypadki:
            tr = bl = ni = bi_ok = bi_all = sp_f = sp_all = sp_ok = 0
            for pl in zbior:
                x, fs = czytaj(pl)
                odc = x if dl is None else x[int(2 * fs): int((2 + dl) * fs)]
                for ton in TONY + [None] + SPOZA:
                    zapisz(p, szyfruj(odc, fs, ton, snr, ct, fw, em), fw)
                    w, pewny, pk, pr = wykryj(p)
                    if ton in SPOZA:
                        trafny = w not in (None, "blad") and abs(w - ton) <= 10
                        sp_all += 1
                        sp_f += pewny and not trafny
                        sp_ok += pewny and trafny
                        if dl is not None:
                            k = spoza_wg_tonu.setdefault((mat, ton), [0, 0])
                            k[0] += pewny and trafny; k[1] += 1
                    elif ton is None:
                        bi_all += 1
                        bi_ok += pewny and w is None
                    elif not pewny:
                        ni += 1
                    elif w == ton:
                        tr += 1
                        wszystkie_przewagi_dobre.append((pk, pr))
                    else:
                        bl += 1
                        wszystkie_zle.append((nazwa, mat, os.path.basename(pl)[:14], ton, w, pk, pr))
            n = tr + bl + ni
            print(f"{nazwa:24s}{mat:12s}{tr:>9d}/{n:<4d}{bl:>14d}{ni:>10d}{bi_ok:>9d}/{bi_all:<3d}{sp_ok:>10d}/{sp_all:<4d}{sp_f:>7d}", flush=True)
    print("tony spoza listy, pewne trafne (wszystkie warianty razem):")
    for (mat, ton), (ok, n) in sorted(spoza_wg_tonu.items()):
        print(f"   {mat:10s} {ton:5d} Hz: {ok}/{n}")
    # Negatywne: sam szum.
    falsz = 0
    for i in range(20):
        zapisz(p, rng.normal(0, 0.1, 48000 * 6), 48000)
        falsz += wykryj(p)[1]
    print(f"sam szum: fałszywie pewne {falsz}/20")
print("błędne pewne:", wszystkie_zle[:15])
