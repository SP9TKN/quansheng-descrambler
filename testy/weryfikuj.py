"""Szyfrowanie w scipy -> odwracanie w Rust -> korelacja z oryginałem w paśmie mowy.
Punkt odniesienia: korelacja dla złego tonu (co 100 Hz obok)."""
import os, subprocess, sys, tempfile, wave
import numpy as np
from math import gcd
from scipy.signal import firwin, lfilter, filtfilt, resample_poly, correlate

sys.stdout.reconfigure(encoding="utf-8")
MOWA, CLI = sys.argv[1], sys.argv[2]


def czytaj(p):
    w = wave.open(p)
    x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(float) / 32768
    return x[:: w.getnchannels()], w.getframerate()


def zapisz(p, x, fs):
    w = wave.open(p, "wb")
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(fs)
    w.writeframes((np.clip(x, -1, 1) * 32767).astype(np.int16).tobytes())
    w.close()


def pasmo(x, fs):
    return filtfilt(firwin(401, [400, 2300], pass_zero=False, fs=fs), 1, x)


def korelacja(a, b, fs):
    n = min(len(a), len(b))
    a, b = pasmo(a[:n], fs), pasmo(b[:n], fs)
    c = correlate(a, b, mode="full", method="fft")
    return float(np.max(np.abs(c)) / np.sqrt(np.sum(a**2) * np.sum(b**2)))


x, fs0 = czytaj(os.path.join(MOWA, "OSR_us_000_0030_8k.wav"))
x = x[int(2 * fs0): int(22 * fs0)]
g = gcd(fs0, 48000)
y = resample_poly(x, 48000 // g, fs0 // g)
y = lfilter(firwin(801, [300, 3000], pass_zero=False, fs=48000), 1, y)
y = y / np.max(np.abs(y)) * 0.5
print(f"{'ton':>6s} {'fs':>6s} {'właściwy':>10s} {'+100 Hz':>9s} {'-100 Hz':>9s}")
with tempfile.TemporaryDirectory() as d:
    we, wy = os.path.join(d, "w.wav"), os.path.join(d, "o.wav")
    for ton in [2600, 3000, 3500]:
        t = np.arange(len(y)) / 48000
        sz = lfilter(firwin(801, 3400, fs=48000), 1, 2 * y * np.cos(2 * np.pi * ton * t))
        sz = sz + 0.02 * np.sin(2 * np.pi * 88.5 * t)
        for fs in [48000, 44100, 8000]:
            g = gcd(48000, fs)
            zapisz(we, resample_poly(sz, fs // g, 48000 // g), fs)
            ref = resample_poly(y, fs // g, 48000 // g)
            wyn = []
            for dt in [0, 100, -100]:
                subprocess.run([CLI, "odwroc", we, wy, "--ton", str(ton + dt)], check=True)
                o, _ = czytaj(wy)
                wyn.append(korelacja(ref, o, fs))
            print(f"{ton:>6d} {fs:>6d} {wyn[0]:>10.3f} {wyn[1]:>9.3f} {wyn[2]:>9.3f}", flush=True)
