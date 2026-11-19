"""Automat jak na żywo na długiej mowie: czy kiedykolwiek przeskakuje na zły ton."""
import glob, os, re, subprocess, sys, tempfile, wave
import numpy as np
from math import gcd
from scipy.signal import firwin, lfilter, resample_poly

sys.stdout.reconfigure(encoding="utf-8")
MOWA, CLI = sys.argv[1], sys.argv[2]
rng = np.random.default_rng(7)


def czytaj(p):
    w = wave.open(p)
    x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(float) / 32768
    return x[:: w.getnchannels()], w.getframerate()


def zapisz(p, x, fs):
    w = wave.open(p, "wb")
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(fs)
    w.writeframes((np.clip(x, -1, 1) * 32767).astype(np.int16).tobytes())
    w.close()


pliki = [p for p in sorted(glob.glob(os.path.join(MOWA, "*.wav"))) if os.path.getsize(p) > 100000]
laczne = zle = opoz_sum = 0
opoz = []
with tempfile.TemporaryDirectory() as d:
    p = os.path.join(d, "t.wav")
    for pl in pliki:
        x, fs0 = czytaj(pl)
        g = gcd(fs0, 48000)
        y = resample_poly(x, 48000 // g, fs0 // g)
        y = lfilter(firwin(801, [300, 3000], pass_zero=False, fs=48000), 1, y)
        y = y / np.max(np.abs(y)) * 0.5
        t = np.arange(len(y)) / 48000
        for ton in range(2600, 3600, 100):
            for snr, ct in [(None, 88.5), (15, 131.8)]:
                s = lfilter(firwin(801, 3400, fs=48000), 1, 2 * y * np.cos(2 * np.pi * ton * t))
                s = s + 0.02 * np.sin(2 * np.pi * ct * t)
                if snr:
                    s = s + rng.normal(0, np.sqrt(np.mean(s**2)) * 10 ** (-snr / 20), len(s))
                zapisz(p, s, 48000)
                r = subprocess.run([CLI, "symuluj", p], capture_output=True, text=True, encoding="utf-8").stdout
                stany = re.findall(r"^\s*([\d.]+) s .*-> ton (.+)$", r, re.M)
                laczne += 1
                pierwszy = next((float(a) for a, b in stany if b != "None"), None)
                zle_st = {b for a, b in stany if b != "None" and b != f"Some(Some({ton}.0))"}
                if pierwszy is None or zle_st:
                    zle += 1
                    print("ZŁE", os.path.basename(pl), ton, snr, "pierwszy", pierwszy, zle_st, flush=True)
                else:
                    opoz.append(pierwszy)
print(f"przebiegi {laczne}, bez żadnego złego tonu {laczne-zle}; czas do 1. decyzji: mediana {np.median(opoz):.1f} s, maks {max(opoz):.1f} s")
